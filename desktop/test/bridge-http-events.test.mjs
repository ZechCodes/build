import assert from "node:assert/strict";
import test from "node:test";
import { createBridgeEventStreams } from "../src/bridge-http-events.mjs";

for (const epoch of [undefined, null, "", 1, "different"]) {
  test(`SSE hello epoch ${String(epoch)} is rejected before producer readiness`, async () => {
    const output = [];
    const owner = {};
    const handle = {
      id: "h",
      owner,
      opening: {
        opening_generation: "generation",
        greeting: { bridge_epoch: "expected" },
      },
      mint: { origin: "https://bridge.test" },
      streams: new Set(),
    };
    const events = createBridgeEventStreams({
      fetch: async () =>
        new Response(
          "event: hello\ndata: " +
            JSON.stringify({
              subscription_id: "subscription",
              opening_generation: "generation",
              durable: false,
              recovery: "current_state_refresh",
              bridge_epoch: epoch,
            }) +
            "\n\n",
          { headers: { "content-type": "text/event-stream" } },
        ),
      check: () => {},
      headers: () => ({}),
      send: (_owner, event) => output.push(event),
      device: async () => ({
        subscription_id: "subscription",
        opening_generation: "generation",
      }),
      handleFor: () => handle,
      retire: () => {},
    });
    try {
      await events.subscribe(
        owner,
        "h",
        { kind: "application" },
        { requestId: "00000000-0000-4000-8000-000000000001" },
      );
      await new Promise(setImmediate);
      assert.equal(
        output.some((event) => event.event === "hello"),
        false,
      );
      assert.throws(() => events.requestHeader(handle, true), {
        code: "stream_required",
      });
    } finally {
      events.clear(handle);
    }
  });
}

test("early event envelopes identify the endpoint before a subscribe reply, including terminal controls", async (t) => {
  const owner = {};
  const handles = new Map(
    ["own", "foreign"].map((id) => [
      id,
      {
        id,
        owner,
        opening: {
          opening_generation: id,
          greeting: { bridge_epoch: "epoch" },
        },
        mint: { origin: "https://bridge.test" },
        streams: new Set(),
      },
    ]),
  );
  const bodies = new Map();
  const output = [];
  let knownStream;
  let releaseReply;
  const replyHeld = new Promise((resolve) => (releaseReply = resolve));
  const events = createBridgeEventStreams({
    fetch: async (url) => {
      const id = url.includes("/foreign/") ? "foreign" : "own";
      return new Response(
        new ReadableStream({
          start(controller) {
            bodies.set(id, controller);
          },
        }),
        { headers: { "content-type": "text/event-stream" } },
      );
    },
    check: () => {},
    headers: () => ({}),
    send: (caller, envelope) => {
      assert.equal(caller, owner);
      output.push(envelope);
      // Another renderer consumer can drain its own stream while this caller
      // still awaits delivery of the subscribe result through IPC.
      if (envelope.handle === "foreign" && envelope.event !== "reset_required")
        events.ack(owner, envelope.stream, envelope.sequence);
    },
    device: async (handle) => ({
      subscription_id: handle.id,
      opening_generation: handle.id,
    }),
    handleFor: (_owner, id) => handles.get(id),
    retire: () => {},
  });
  t.after(() => {
    for (const handle of handles.values()) events.clear(handle);
  });
  const pending = events
    .subscribe(
      owner,
      "own",
      { kind: "application" },
      { requestId: "00000000-0000-4000-8000-000000000001" },
    )
    .then(async (result) => {
      await replyHeld;
      knownStream = result.stream;
      return result;
    });
  await events.subscribe(
    owner,
    "foreign",
    { kind: "application" },
    { requestId: "00000000-0000-4000-8000-000000000002" },
  );
  await new Promise(setImmediate);
  const emit = (id, event, data) =>
    bodies
      .get(id)
      .enqueue(
        new TextEncoder().encode(
          "event: " + event + "\ndata: " + JSON.stringify(data) + "\n\n",
        ),
      );
  for (const id of handles.keys())
    emit(id, "hello", {
      subscription_id: id,
      opening_generation: id,
      bridge_epoch: "epoch",
      durable: false,
      recovery: "current_state_refresh",
    });
  for (let i = 0; i < 128; i++) emit("foreign", "changes", { index: i });
  await new Promise(setImmediate);
  assert.equal(knownStream, undefined);
  assert.equal(
    output.filter((message) => message.handle === "foreign").length,
    129,
  );
  const ownEarly = output.filter((message) => message.handle === "own");
  assert.equal(ownEarly.length, 1);
  assert.equal(ownEarly[0].event, "hello");
  assert.equal(ownEarly[0].sequence, 1);
  assert.equal(ownEarly[0].requestId, "00000000-0000-4000-8000-000000000001");
  releaseReply();
  const { stream } = await pending;
  assert.equal(ownEarly[0].stream, stream);
  events.clear(handles.get("own"), "credentials_closed");
  assert.deepEqual(output.at(-1), {
    handle: "own",
    requestId: "00000000-0000-4000-8000-000000000001",
    stream,
    sequence: 2,
    event: "reset_required",
    data: { reason: "credentials_closed", recovery: "current_state_refresh" },
  });
});

test("same-handle replacement keeps old data and terminal controls tied to their original attempt", async (t) => {
  const owner = {};
  const output = [];
  const bodies = new Map();
  let issued = 0;
  const handle = {
    id: "endpoint",
    owner,
    opening: {
      opening_generation: "opening",
      greeting: { bridge_epoch: "epoch" },
    },
    mint: { origin: "https://bridge.test" },
    streams: new Set(),
  };
  const events = createBridgeEventStreams({
    fetch: async (url) =>
      new Response(
        new ReadableStream({
          start(controller) {
            bodies.set(url.split("/").at(-2), controller);
          },
        }),
        { headers: { "content-type": "text/event-stream" } },
      ),
    check: () => {},
    headers: () => ({}),
    send: (_owner, envelope) => output.push(envelope),
    device: async () => ({
      subscription_id: `wire${++issued}`,
      opening_generation: "opening",
    }),
    handleFor: () => handle,
    retire: () => {},
  });
  t.after(() => events.clear(handle));
  const firstId = "00000000-0000-4000-8000-000000000001";
  const secondId = "00000000-0000-4000-8000-000000000002";
  const first = await events.subscribe(
    owner,
    handle.id,
    { kind: "application" },
    { requestId: firstId },
  );
  await new Promise(setImmediate);
  const hello = (id) =>
    "event: hello\ndata: " +
    JSON.stringify({
      subscription_id: id,
      opening_generation: "opening",
      bridge_epoch: "epoch",
      durable: false,
      recovery: "current_state_refresh",
    }) +
    "\n\n";
  bodies.get("wire1").enqueue(new TextEncoder().encode(hello("wire1")));
  await new Promise(setImmediate);
  bodies
    .get("wire1")
    .enqueue(
      new TextEncoder().encode(
        'event: closing\ndata: {"reason":"server_shutdown"}\n\n',
      ),
    );
  await new Promise(setImmediate);
  events.clear(handle, "disconnected");
  const old = output.slice();
  const second = await events.subscribe(
    owner,
    handle.id,
    { kind: "application" },
    { requestId: secondId },
  );
  await new Promise(setImmediate);
  bodies.get("wire2").enqueue(new TextEncoder().encode(hello("wire2")));
  await new Promise(setImmediate);
  assert.notEqual(first.stream, second.stream);
  assert.equal(old.length, 3);
  assert.equal(old[1].event, "closing");
  for (const envelope of old) {
    assert.equal(envelope.handle, handle.id);
    assert.equal(envelope.requestId, firstId);
    assert.equal(envelope.stream, first.stream);
  }
  assert.equal(old.at(-1).event, "reset_required");
  assert.equal(output.at(-1).requestId, secondId);
  assert.equal(output.at(-1).stream, second.stream);
  assert.equal(output.at(-1).sequence, 1);
});

test("subscribe correlation is required, bounded, immutable and cannot be reused on the same handle", async (t) => {
  const owner = {};
  const handle = {
    id: "endpoint",
    owner,
    opening: { opening_generation: "opening" },
    mint: { origin: "https://bridge.test" },
    streams: new Set(),
  };
  let requests = 0;
  const events = createBridgeEventStreams({
    fetch: async () =>
      new Response(new ReadableStream({ start() {} }), {
        headers: { "content-type": "text/event-stream" },
      }),
    check: () => {},
    headers: () => ({}),
    send: () => {},
    device: async () => {
      requests++;
      return {
        subscription_id: `wire${requests}`,
        opening_generation: "opening",
      };
    },
    handleFor: () => handle,
    retire: () => {},
  });
  t.after(() => events.clear(handle));
  for (const options of [
    undefined,
    {},
    { requestId: "not-uuid" },
    { requestId: "00000000-0000-4000-8000-000000000001", headers: {} },
  ])
    await assert.rejects(
      events.subscribe(owner, handle.id, { kind: "application" }, options),
      { code: "invalid_options" },
    );
  assert.equal(requests, 0);
  const id = "00000000-0000-4000-8000-000000000001";
  await events.subscribe(
    owner,
    handle.id,
    { kind: "application" },
    { requestId: id },
  );
  events.clear(handle);
  await assert.rejects(
    events.subscribe(
      owner,
      handle.id,
      { kind: "application" },
      { requestId: id },
    ),
    { code: "duplicate_request" },
  );
  assert.equal(requests, 1);
});

test("reuse guard retains all protected attempt IDs and refuses capacity without eviction", async (t) => {
  const owner = {};
  const handle = {
    id: "endpoint",
    owner,
    opening: { opening_generation: "opening" },
    mint: { origin: "https://bridge.test" },
    streams: new Set(),
  };
  let requests = 0;
  const events = createBridgeEventStreams({
    fetch: async () => {
      throw new Error("unexpected pump");
    },
    check: () => {},
    headers: () => ({}),
    send: () => {},
    device: async () => ({
      subscription_id: `wire${++requests}`,
      opening_generation: "opening",
    }),
    handleFor: () => handle,
    retire: () => {},
  });
  t.after(() => events.clear(handle));
  const id = (index) =>
    `00000000-0000-4000-8000-${index.toString(16).padStart(12, "0")}`;
  for (let index = 1; index <= 128; index++) {
    await events.subscribe(
      owner,
      handle.id,
      { kind: "application" },
      { requestId: id(index) },
    );
    events.clear(handle);
  }
  await assert.rejects(
    events.subscribe(
      owner,
      handle.id,
      { kind: "application" },
      { requestId: id(129) },
    ),
    { code: "busy" },
  );
  await assert.rejects(
    events.subscribe(
      owner,
      handle.id,
      { kind: "application" },
      { requestId: id(1) },
    ),
    { code: "duplicate_request" },
  );
  assert.equal(requests, 128);
});

test("failed subscription attempt cannot be reused before handle retirement completes", async () => {
  const owner = {};
  const handle = {
    id: "endpoint",
    owner,
    opening: { opening_generation: "opening" },
    mint: { origin: "https://bridge.test" },
    streams: new Set(),
  };
  let retired = 0;
  let requests = 0;
  const events = createBridgeEventStreams({
    fetch: async () => {},
    check: () => {},
    headers: () => ({}),
    send: () => {},
    device: async () => {
      requests++;
      throw new Error("offline");
    },
    handleFor: () => handle,
    retire: () => retired++,
  });
  const options = { requestId: "00000000-0000-4000-8000-000000000001" };
  await assert.rejects(
    events.subscribe(owner, handle.id, { kind: "application" }, options),
  );
  assert.equal(retired, 1);
  await assert.rejects(
    events.subscribe(owner, handle.id, { kind: "application" }, options),
    { code: "duplicate_request" },
  );
  assert.equal(requests, 1);
});
