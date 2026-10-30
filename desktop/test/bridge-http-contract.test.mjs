import assert from "node:assert/strict";
import test from "node:test";
import { requestOptions, buildRoute } from "../src/bridge-http-contract.mjs";
const operation = "00000000-0000-4000-8000-000000000001";

test("write metadata preserves absent versus empty dependencies and rejects broader shapes", () => {
  assert.equal(
    requestOptions({ write: { operation_id: operation } })["X-Build-After"],
    undefined,
  );
  assert.equal(
    requestOptions({ write: { operation_id: operation, after: [] } })[
      "X-Build-After"
    ],
    "[]",
  );
  for (const after of [
    null,
    {},
    "header",
    new Array(9).fill(operation),
    ["not-a-uuid"],
  ]) {
    assert.throws(
      () => requestOptions({ write: { operation_id: operation, after } }),
      { code: "invalid_options" },
    );
  }
  assert.throws(() => requestOptions({ requestId: "bad\r\nHeader: value" }), {
    code: "invalid_options",
  });
  assert.equal(
    requestOptions({ requestId: "x".repeat(128) })["X-Build-Request-Id"].length,
    128,
  );
  assert.throws(() => requestOptions({ requestId: "x".repeat(129) }), {
    code: "invalid_options",
  });
  assert.throws(() => requestOptions({ requestId: "emoji-😀" }), {
    code: "invalid_options",
  });
});

test("route maxima are trusted manifest limits; renderer cannot replace paths or body contracts", () => {
  const operations = [
    {
      operationId: "upload",
      verb: "POST",
      path: "/v1/projects/{project_id}/attachments",
      pathParams: ["project_id"],
      body: "remaining",
      requestMaxBytes: 7056044,
      responseMaxBytes: 8388608,
      requires_events: true,
    },
  ];
  const route = buildRoute({ operations }, "upload", {
    project_id: "p",
    content_b64: "abc",
  });
  assert.equal(route.maxBody, 7056044);
  assert.equal(route.maxBytes, 8388608);
  assert.equal(route.requiresEvents, true);
  assert.deepEqual(route.body, { content_b64: "abc" });
  assert.throws(() => buildRoute({ operations }, "https://evil.test", {}), {
    code: "unsupported_operation",
  });
});

test("unsupported query placement fails closed; path/body mapping never drops parameters", () => {
  const base = {
    operationId: "read",
    verb: "GET",
    path: "/v1/items/{item}",
    pathParams: ["item"],
    queryParams: [],
    body: "none",
    responseMaxBytes: 1024,
  };
  assert.throws(
    () =>
      buildRoute(
        { operations: [{ ...base, queryParams: ["cursor", "limit"] }] },
        "read",
        { item: "x", cursor: "a & b", limit: 25 },
      ),
    { code: "invalid_manifest" },
  );
  assert.throws(
    () =>
      buildRoute({ operations: [base] }, "read", { item: "x", cursor: "next" }),
    { code: "invalid_params" },
  );
  const read = buildRoute({ operations: [base] }, "read", { item: "x & y" });
  assert.equal(read.path, "/v1/items/x%20%26%20y");
  assert.equal(read.body, undefined);
  const query = buildRoute(
    { operations: [{ ...base, verb: "POST", body: "remaining" }] },
    "read",
    { item: "x", cursor: "a & b", limit: 25 },
  );
  assert.equal(query.path, "/v1/items/x");
  assert.deepEqual(query.body, { cursor: "a & b", limit: 25 });
});
