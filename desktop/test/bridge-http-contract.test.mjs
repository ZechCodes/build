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
