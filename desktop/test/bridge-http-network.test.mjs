import assert from "node:assert/strict";
import test from "node:test";
import {
  canonicalOrigin,
  jsonRequest,
  consumeEvents,
} from "../src/bridge-http-network.mjs";

const json = (body, init = {}) =>
  new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
    ...init,
  });

test("canonical HTTPS origins reject ambiguous URLs before networking", () => {
  assert.equal(
    canonicalOrigin("https://EXAMPLE.com:443/"),
    "https://example.com",
  );
  assert.equal(canonicalOrigin("https://[::1]:8443"), "https://[::1]:8443");
  for (const url of [
    "http://localhost",
    "https:host",
    "https://host:",
    "https://u:p@host",
    "https://host/path",
    "https://host?x",
    "https://host#x",
    "https://host.",
    "https://127.1",
    "https://0x7f000001",
    "https://host\\foo",
    "https://host/%2e",
    " https://host",
    "https://host:0",
  ]) {
    assert.throws(() => canonicalOrigin(url), { code: "invalid_origin" }, url);
  }
});

test("JSON requests omit cookies, reject redirects and use bounded bodies", async () => {
  let seen;
  const fetch = async (url, options) => {
    seen = { url, options };
    return json({ answer: 42 });
  };
  assert.deepEqual(
    await jsonRequest(fetch, "https://bridge.test/v1/sessions", {
      method: "POST",
      body: { api_range: "1" },
      maxBytes: 128,
    }),
    { answer: 42 },
  );
  assert.equal(seen.options.credentials, "omit");
  assert.equal(seen.options.redirect, "error");
  assert.equal(seen.options.cache, "no-store");
  assert.equal(seen.options.referrerPolicy, "no-referrer");
  await assert.rejects(
    jsonRequest(fetch, "https://bridge.test", {
      body: { large: "x".repeat(100) },
      maxBody: 32,
    }),
    { code: "request_too_large" },
  );
  await assert.rejects(
    jsonRequest(
      async () => json({ large: "x".repeat(100) }),
      "https://bridge.test",
      { maxBytes: 32 },
    ),
    { code: "response_too_large" },
  );
  await assert.rejects(
    jsonRequest(
      async () =>
        new Response(null, {
          status: 302,
          headers: { location: "https://evil.test" },
        }),
      "https://bridge.test",
    ),
    { code: "http_error" },
  );
});

test("network errors and untrusted server bodies never become credential-bearing errors", async () => {
  const secret = "fixture-secret-must-not-escape";
  await assert.rejects(
    jsonRequest(async () => {
      throw new Error(secret);
    }, "https://bridge.test"),
    (error) =>
      error.code === "network_error" && !String(error).includes(secret),
  );
  await assert.rejects(
    jsonRequest(
      async () => json({ error: secret }, { status: 401 }),
      "https://bridge.test",
    ),
    (error) => error.code === "unauthorized" && !String(error).includes(secret),
  );
});

test("deadlines abort fetch through body consumption", async () => {
  let signal;
  await assert.rejects(
    jsonRequest(
      async (_url, options) => {
        signal = options.signal;
        return new Response(
          new ReadableStream({
            start(controller) {
              signal.addEventListener(
                "abort",
                () => controller.error(new Error("aborted")),
                { once: true },
              );
            },
          }),
          { headers: { "content-type": "application/json" } },
        );
      },
      "https://bridge.test",
      { timeoutMs: 10 },
    ),
    { code: "timeout" },
  );
  assert.equal(signal.aborted, true);
});

const bytes = (value) => new TextEncoder().encode(value);
function eventResponse(chunks) {
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(bytes(chunk));
        controller.close();
      },
    }),
    { headers: { "content-type": "text/event-stream" } },
  );
}

test("maintained SSE parser handles fragmented UTF8, CRLF and heartbeat without durable cursors", async () => {
  const events = [];
  await consumeEvents(
    eventResponse([
      ": heartbeat\r",
      '\n\r\nevent: changes\r\ndata: {"name":"café"}\r',
      "\n\r\n",
    ]),
    (event) => events.push(event),
  );
  assert.deepEqual(events, [{ event: "changes", data: { name: "café" } }]);
  await assert.rejects(
    consumeEvents(
      eventResponse(["id: cursor\nevent: changes\ndata: {}\n\n"]),
      () => {},
    ),
    { code: "invalid_stream" },
  );
});

test("SSE rejects malformed and oversized unfinished events", async () => {
  await assert.rejects(
    consumeEvents(eventResponse(["event: changes\ndata: nope\n\n"]), () => {}),
    { code: "invalid_stream" },
  );
  await assert.rejects(
    consumeEvents(
      eventResponse(["data: " + "x".repeat(1024 * 1024 + 1)]),
      () => {},
    ),
    { code: "stream_too_large" },
  );
  await assert.rejects(
    consumeEvents(json({}), () => {}),
    { code: "invalid_stream" },
  );
});

test("typed RPC domain errors survive HTTP mapping while auth errors stay native", async () => {
  const envelope = {
    id: "r",
    ok: false,
    error: "Task changed",
    error_code: "conflict",
    retryable: false,
    bridge_epoch: 1,
    request_id: "r",
  };
  assert.deepEqual(
    await jsonRequest(
      async () => json(envelope, { status: 409 }),
      "https://bridge.test",
      { apiEnvelope: true },
    ),
    envelope,
  );
  await assert.rejects(
    jsonRequest(
      async () => json(envelope, { status: 401 }),
      "https://bridge.test",
      { apiEnvelope: true },
    ),
    { code: "unauthorized" },
  );
});
