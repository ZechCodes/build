import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";
import test from "node:test";
import { createBridgeHttp } from "../src/bridge-http.mjs";

const mint = (overrides) => ({
  issuer: "https://getbuild.ing",
  account_id: "account",
  device_id: "device",
  origin: "https://bridge.test",
  endpoint_epoch: "epoch",
  family_id: "family",
  access_token: "fixture-device-token",
  expires_at: Math.floor(Date.now() / 1000) + 300,
  scope: "bridge:read bridge:write",
  name: "Bridge",
  fingerprint: "fingerprint",
  ...overrides,
});
const opening = () => ({
  session_id: "session",
  opening_generation: "generation",
  grant_expires_at: new Date(Date.now() + 30000).toISOString(),
  greeting: { capabilities: [], bridge_epoch: "epoch" },
  transport: {
    resource_manifest_version: 1,
    operation_ids: ["listTasks"],
    terminal: "webrtc",
    durable_events: false,
  },
});
const json = (value, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
function harness(t, options = {}) {
  const requests = [];
  const output = [];
  const owner = { generation: 1, alive: true };
  const auth = { generation: 1, accessToken: "fixture-hosted-token" };
  const fetch = async (url, opts) => {
    requests.push({ url, opts });
    if (options.fetch) {
      const result = await options.fetch(url, opts);
      if (result) return result;
    }
    if (url.endsWith("/challenges"))
      return json({
        challenge_id: "challenge",
        nonce: "n".repeat(32),
        issuer: "https://getbuild.ing",
        expires_at: Math.floor(Date.now() / 1000) + 60,
      });
    if (url.endsWith("/.well-known/build-bridge"))
      return json({ proof: "fixture-proof" });
    if (url.endsWith("/complete") || url.endsWith("/http-credentials"))
      return json(mint(options.mint));
    if (url.endsWith("/v1/sessions")) return json(opening(), 201);
    if (url.endsWith("/v1/subscriptions"))
      return json({ subscription_id: "subscription", durable: false }, 201);
    if (opts.method === "DELETE") return new Response(null, { status: 204 });
    return json({ result: { tasks: [] } });
  };
  const bridge = createBridgeHttp({
    fetch,
    getAuth: () => auth,
    isOwnerCurrent: (o) => o.alive,
    send: (o, event) => output.push({ o, event }),
    manifest: {
      version: 1,
      operations: [
        {
          method: "tasks.list",
          operationId: "listTasks",
          verb: "POST",
          path: "/v1/projects/{project_id}/task-queries",
          pathParams: ["project_id"],
          queryParams: [],
          body: "remaining",
          responseMaxBytes: 1024,
          scope: "bridge:read",
        },
      ],
    },
  });
  t.after(() => bridge.clear());
  return { bridge, owner, auth, requests, output };
}

test("discovery sends hosted OAuth only to hosted routes; device token remains private", async (t) => {
  const h = harness(t);
  const identity = await h.bridge.discover(h.owner, "https://bridge.test");
  assert.deepEqual(Object.keys(identity).sort(), [
    "deviceId",
    "endpointEpoch",
    "familyId",
    "handle",
    "origin",
  ]);
  assert.equal(JSON.stringify(identity).includes("token"), false);
  assert.equal(
    h.requests[0].opts.headers.Authorization,
    "Bearer fixture-hosted-token",
  );
  assert.equal(h.requests[1].opts.headers.Authorization, undefined);
  assert.equal(
    h.requests[2].opts.headers.Authorization,
    "Bearer fixture-hosted-token",
  );
  await h.bridge.open(h.owner, identity.handle, "1");
  assert.equal(
    h.requests[3].opts.headers.Authorization,
    "Bearer fixture-device-token",
  );
});

test("substituted endpoint and account renewal bindings are refused before bridge credentials", async (t) => {
  const h = harness(t, { mint: { origin: "https://other.test" } });
  await assert.rejects(h.bridge.discover(h.owner, "https://bridge.test"), {
    code: "binding_mismatch",
  });
  assert.equal(
    h.requests.some(
      (r) => r.opts.headers.Authorization === "Bearer fixture-device-token",
    ),
    false,
  );
  await assert.rejects(
    h.bridge.resume(h.owner, {
      deviceId: "other",
      origin: "https://bridge.test",
      familyId: "family",
      endpointEpoch: "epoch",
    }),
    { code: "binding_mismatch" },
  );
});

test("handles belong to exact owner and credential generation; late discovery cannot resurrect", async (t) => {
  const h = harness(t);
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await assert.rejects(h.bridge.open({ ...h.owner }, handle, "1"), {
    code: "invalid_handle",
  });
  h.auth.generation++;
  await assert.rejects(h.bridge.open(h.owner, handle, "1"), {
    code: "stale_owner",
  });
  let release;
  const wait = new Promise((resolve) => (release = resolve));
  const late = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/complete")) {
        await wait;
        return json(mint());
      }
    },
  });
  const pending = late.bridge.discover(late.owner, "https://bridge.test");
  await new Promise(setImmediate);
  late.bridge.clear();
  release();
  await assert.rejects(pending, { code: "canceled" });
});

test("request resolver forbids proxy overrides, validates identifiers and preserves typed params", async (t) => {
  const h = harness(t);
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  await assert.rejects(
    h.bridge.request(h.owner, handle, "https://evil.test", {}),
    { code: "unsupported_operation" },
  );
  await assert.rejects(
    h.bridge.request(
      h.owner,
      handle,
      "listTasks",
      { project_id: "p" },
      { headers: { Authorization: "bad" } },
    ),
    { code: "invalid_options" },
  );
  await assert.rejects(
    h.bridge.request(h.owner, handle, "listTasks", { project_id: ".." }),
    { code: "invalid_params" },
  );
  assert.deepEqual(
    await h.bridge.request(h.owner, handle, "listTasks", {
      project_id: "p/a",
      filters: ["open"],
    }),
    { result: { tasks: [] } },
  );
  const last = h.requests.at(-1);
  assert.equal(last.url, "https://bridge.test/v1/projects/p%2Fa/task-queries");
  assert.deepEqual(JSON.parse(last.opts.body), { filters: ["open"] });
  assert.equal(last.opts.headers["X-Build-Session"], "session");
});

test("cancel and signout abort outstanding work and drop late replies", async (t) => {
  let release;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.includes("task-queries"))
        return new Promise((resolve) => (release = resolve));
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const pending = h.bridge.request(
    h.owner,
    handle,
    "listTasks",
    { project_id: "p" },
    { requestId: "request" },
  );
  await new Promise(setImmediate);
  h.bridge.cancel(h.owner, "request");
  assert.equal(h.requests.at(-1).opts.signal.aborted, true);
  release(json({ result: "late" }));
  await assert.rejects(pending, { code: "canceled" });
  h.bridge.clear();
  await assert.rejects(
    h.bridge.request(h.owner, handle, "listTasks", { project_id: "p" }),
    { code: "invalid_handle" },
  );
});

test("authenticated opening must advertise compatible complete manifest and explicit terminal policy", async (t) => {
  for (const transport of [
    undefined,
    {
      resource_manifest_version: 2,
      operation_ids: ["listTasks"],
      terminal: "webrtc",
      durable_events: false,
    },
    {
      resource_manifest_version: 1,
      operation_ids: [],
      terminal: "webrtc",
      durable_events: false,
    },
  ]) {
    const h = harness(t, {
      fetch: async (url) =>
        url.endsWith("/v1/sessions")
          ? json({ ...opening(), transport })
          : undefined,
    });
    const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
    await assert.rejects(h.bridge.open(h.owner, handle, "1"), {
      code: "incompatible_transport",
    });
  }
});

test("independent hard expiry prevents late in-flight output and closes handle", async (t) => {
  let release;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/sessions"))
        return json({
          ...opening(),
          grant_expires_at: new Date(Date.now() + 40).toISOString(),
        });
      if (url.includes("task-queries"))
        return new Promise((resolve) => (release = resolve));
      if (url.endsWith("/v1/sessions/session")) return new Promise(() => {});
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const pending = assert.rejects(
    h.bridge.request(h.owner, handle, "listTasks", { project_id: "p" }),
  );
  await new Promise((resolve) => setTimeout(resolve, 60));
  release(json({ result: "late" }));
  await pending;
  await assert.rejects(
    h.bridge.request(h.owner, handle, "listTasks", { project_id: "p" }),
    { code: "invalid_handle" },
  );
});

test("stream local flow control bounds unacknowledged events and fences post-overflow output", async (t) => {
  let controller;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return json({
          subscription_id: "subscription",
          opening_generation: "generation",
          events_path: "/v1/subscriptions/subscription/events",
        });
      if (url.endsWith("/events"))
        return new Response(
          new ReadableStream({
            start(c) {
              controller = c;
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const { stream } = await h.bridge.subscribe(
    h.owner,
    handle,
    {
      kind: "application",
    },
    { requestId: randomUUID() },
  );
  await new Promise(setImmediate);
  const emit = (data) => controller.enqueue(new TextEncoder().encode(data));
  emit(
    'event: hello\ndata: {"durable":false,"subscription_id":"subscription","recovery":"current_state_refresh","opening_generation":"generation","bridge_epoch":"epoch"}\n\n',
  );
  await new Promise(setImmediate);
  h.bridge.ack(h.owner, stream, 1);
  for (let i = 0; i < 129; i++) emit("event: changes\ndata: {}\n\n");
  await new Promise(setImmediate);
  assert.equal(h.output.length, 130);
  assert.equal(h.output.at(-1).event.event, "reset_required");
  assert.equal(h.output.at(-1).event.data.reason, "overflow");
  await assert.rejects(h.bridge.unsubscribe(h.owner, stream), {
    code: "invalid_stream",
  });
});

test("resource calls capture application stream handle and cannot run requires-events before hello", async (t) => {
  let controller;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return json({
          subscription_id: "subscription",
          opening_generation: "generation",
        });
      if (url.endsWith("/events"))
        return new Response(
          new ReadableStream({
            start(c) {
              controller = c;
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const { stream } = await h.bridge.subscribe(
    h.owner,
    handle,
    {
      kind: "application",
    },
    { requestId: randomUUID() },
  );
  await new Promise(setImmediate);
  controller.enqueue(
    new TextEncoder().encode(
      'event: hello\ndata: {"durable":false,"subscription_id":"subscription","recovery":"current_state_refresh","opening_generation":"generation","bridge_epoch":"epoch"}\n\n',
    ),
  );
  await new Promise(setImmediate);
  await h.bridge.request(h.owner, handle, "listTasks", { project_id: "p" });
  assert.equal(
    h.requests.at(-1).opts.headers["X-Build-Subscription"],
    "subscription",
  );
  h.bridge.ack(h.owner, stream, 1);
});

test("stream reconnect POST retains predecessor after exact unsubscribe and does not rewrite requests", async (t) => {
  let counter = 0;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return json({
          subscription_id: `sub${++counter}`,
          opening_generation: "generation",
        });
      if (url.endsWith("/events"))
        return new Response(new ReadableStream({ start() {} }), {
          headers: { "content-type": "text/event-stream" },
        });
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const first = await h.bridge.subscribe(
    h.owner,
    handle,
    {
      kind: "application",
    },
    { requestId: randomUUID() },
  );
  await new Promise(setImmediate);
  await h.bridge.unsubscribe(h.owner, first.stream);
  await h.bridge.subscribe(
    h.owner,
    handle,
    { kind: "application" },
    { requestId: randomUUID() },
  );
  const posts = h.requests.filter((r) => r.url.endsWith("/v1/subscriptions"));
  assert.equal(posts[0].opts.headers["X-Build-Subscription"], undefined);
  assert.equal(posts[1].opts.headers["X-Build-Subscription"], "sub1");
});

test("creation response loss retires opening rather than replaying application POST", async (t) => {
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions")) throw new Error("lost");
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  await assert.rejects(
    h.bridge.subscribe(
      h.owner,
      handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
  );
  await assert.rejects(
    h.bridge.subscribe(
      h.owner,
      handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
    { code: "invalid_handle" },
  );
  assert.equal(
    h.requests.filter((r) => r.url.endsWith("/v1/subscriptions")).length,
    1,
  );
});

test("pending reads and writes reserve bounded aggregate response bytes; metadata stays finite", async (t) => {
  let release;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.includes("task-queries"))
        return new Promise((resolve) => (release = resolve));
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const pending = h.bridge.request(
    h.owner,
    handle,
    "listTasks",
    { project_id: "p" },
    {
      requestId: "pending",
      priority: "background",
      write: {
        operation_id: "00000000-0000-4000-8000-000000000001",
        after: [],
      },
    },
  );
  await new Promise(setImmediate);
  const request = h.requests.at(-1);
  assert.equal(request.opts.headers["X-Build-Priority"], "background");
  assert.equal(
    request.opts.headers["Idempotency-Key"],
    "00000000-0000-4000-8000-000000000001",
  );
  assert.equal(request.opts.headers["X-Build-After"], "[]");
  await assert.rejects(
    h.bridge.request(
      h.owner,
      handle,
      "listTasks",
      { project_id: "p" },
      { priority: "user" },
    ),
    { code: "invalid_options" },
  );
  h.bridge.cancel(h.owner, "pending");
  const errorCheck = assert.rejects(
    pending,
    (error) => error.code === "canceled" && error.uncertain === true,
  );
  release(json({ result: "late" }));
  await errorCheck;
});

test("credential rotation fences first, revokes only tracked families with old OAuth, and cannot resurrect offline", async (t) => {
  const h = harness(t, {
    fetch: async (url, opts) => {
      if (url.includes("/http-credentials/") && opts.method === "DELETE")
        throw new Error("offline");
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  const retiring = h.bridge.revoke("fixture-old-hosted-token");
  await assert.rejects(h.bridge.open(h.owner, handle, "1"), {
    code: "invalid_handle",
  });
  const result = await retiring;
  assert.equal(result.failed, 1);
  const deletion = h.requests.find((r) => r.opts.method === "DELETE");
  assert.equal(
    deletion.url,
    "https://getbuild.ing/api/devices/device/http-credentials/family",
  );
  assert.equal(
    deletion.opts.headers.Authorization,
    "Bearer fixture-old-hosted-token",
  );
  assert.equal((await h.bridge.revoke("other")).failed, 0);
});

test("near-expiry derived credential caps opening and stream deadline without grace", async (t) => {
  const h = harness(t, { mint: { expires_at: (Date.now() + 60) / 1000 } });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  await new Promise((resolve) => setTimeout(resolve, 80));
  await assert.rejects(
    h.bridge.request(h.owner, handle, "listTasks", { project_id: "p" }),
    { code: "invalid_handle" },
  );
});

test("revocation of one native account context cannot close another context or revoke its family", async (t) => {
  const one = harness(t);
  const two = harness(t, { mint: { family_id: "other-family" } });
  const first = await one.bridge.discover(one.owner, "https://bridge.test");
  const second = await two.bridge.discover(two.owner, "https://bridge.test");
  await one.bridge.revoke("first-account-issuance");
  await assert.rejects(one.bridge.open(one.owner, first.handle, "1"), {
    code: "invalid_handle",
  });
  assert.ok(await two.bridge.open(two.owner, second.handle, "1"));
  assert.equal(
    two.requests.some((r) => r.opts.method === "DELETE"),
    false,
  );
  assert.equal(
    one.requests.some((r) => r.url.includes("other-family")),
    false,
  );
});

test("unacknowledged stream closes after local stalled-delivery deadline and expiry emits recovery", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let controller;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return json({
          subscription_id: "subscription",
          opening_generation: "generation",
        });
      if (url.endsWith("/events"))
        return new Response(
          new ReadableStream({
            start(c) {
              controller = c;
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  await h.bridge.subscribe(
    h.owner,
    handle,
    { kind: "application" },
    { requestId: randomUUID() },
  );
  await new Promise(setImmediate);
  controller.enqueue(
    new TextEncoder().encode(
      'event: hello\ndata: {"subscription_id":"subscription","durable":false,"recovery":"current_state_refresh","opening_generation":"generation","bridge_epoch":"epoch"}\n\n',
    ),
  );
  await new Promise(setImmediate);
  t.mock.timers.tick(10001);
  await new Promise(setImmediate);
  assert.equal(h.output.at(-1).event.data.reason, "stalled");
});

test("byte cap bounds multibyte SSE output independently of event count", async (t) => {
  let controller;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return json({
          subscription_id: "subscription",
          opening_generation: "generation",
        });
      if (url.endsWith("/events"))
        return new Response(
          new ReadableStream({
            start(c) {
              controller = c;
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        );
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  await h.bridge.subscribe(
    h.owner,
    handle,
    { kind: "application" },
    { requestId: randomUUID() },
  );
  await new Promise(setImmediate);
  const send = (value) => controller.enqueue(new TextEncoder().encode(value));
  send(
    'event: hello\ndata: {"subscription_id":"subscription","durable":false,"recovery":"current_state_refresh","opening_generation":"generation","bridge_epoch":"epoch"}\n\n',
  );
  for (let i = 0; i < 3; i++)
    send(
      "event: changes\ndata: " +
        JSON.stringify({ text: "é".repeat(230000) }) +
        "\n\n",
    );
  await new Promise(setImmediate);
  assert.equal(h.output.at(-1).event.data.reason, "overflow");
  assert.equal(h.output.length, 4);
});

test("concurrent subscriptions on one handle refuse replacement and capture immutable correlation", async (t) => {
  let release;
  const h = harness(t, {
    fetch: async (url) => {
      if (url.endsWith("/v1/subscriptions"))
        return new Promise((resolve) => (release = resolve));
      if (url.endsWith("/events"))
        return new Response(
          'event: hello\ndata: {"subscription_id":"subscription","opening_generation":"generation","bridge_epoch":"epoch","durable":false,"recovery":"current_state_refresh"}\n\n',
          { headers: { "content-type": "text/event-stream" } },
        );
    },
  });
  const { handle } = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, handle, "1");
  const firstId = randomUUID();
  const options = { requestId: firstId };
  const pending = h.bridge.subscribe(
    h.owner,
    handle,
    { kind: "application" },
    options,
  );
  await new Promise(setImmediate);
  await assert.rejects(
    h.bridge.subscribe(
      h.owner,
      handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
    { code: "already_subscribed" },
  );
  assert.equal(
    h.requests.filter((r) => r.url.endsWith("/v1/subscriptions")).length,
    1,
  );
  options.requestId = randomUUID();
  release(
    json({ subscription_id: "subscription", opening_generation: "generation" }),
  );
  const { stream } = await pending;
  await new Promise(setImmediate);
  assert.ok(h.output.length >= 1);
  for (const { event } of h.output) {
    assert.equal(event.handle, handle);
    assert.equal(event.requestId, firstId);
    assert.equal(event.stream, stream);
  }
});

test("retired owner handles never authorize a new opening or subscription", async (t) => {
  const h = harness(t);
  const first = await h.bridge.discover(h.owner, "https://bridge.test");
  await h.bridge.open(h.owner, first.handle, "1");
  await assert.rejects(
    h.bridge.subscribe(
      { ...h.owner },
      first.handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
    { code: "invalid_handle" },
  );
  h.bridge.clear(h.owner);
  h.owner.alive = false;
  const successor = { alive: true, id: 2 };
  const second = await h.bridge.resume(successor, {
    deviceId: "device",
    origin: "https://bridge.test",
    familyId: "family",
    endpointEpoch: "epoch",
  });
  assert.notEqual(first.handle, second.handle);
  await h.bridge.open(successor, second.handle, "1");
  await assert.rejects(
    h.bridge.subscribe(
      h.owner,
      second.handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
    { code: "invalid_handle" },
  );
  await assert.rejects(
    h.bridge.subscribe(
      successor,
      first.handle,
      { kind: "application" },
      { requestId: randomUUID() },
    ),
    { code: "invalid_handle" },
  );
  assert.equal(
    h.requests.some((r) => r.url.endsWith("/v1/subscriptions")),
    false,
  );
});
