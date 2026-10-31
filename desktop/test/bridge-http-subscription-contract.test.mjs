import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createBridgeEventStreams } from "../src/bridge-http-events.mjs";
import { jsonRequest } from "../src/bridge-http-network.mjs";

// The bridge writes this fixture from its own responses
// (bridge/src/http/runtime/tests/subscriptions/contract.rs).
const ids = {
  session_id: "c0a80001-0000-4000-8000-000000000001",
  subscription_id: "c0a80001-0000-4000-8000-000000000002",
  opening_generation: "c0a80001-0000-4000-8000-000000000003",
  bridge_epoch: "c0a80001-0000-4000-8000-000000000004",
};
const contract = JSON.parse(
  readFileSync(
    new URL(
      "../../fixtures/api/http/subscription-contract.json",
      import.meta.url,
    ),
    "utf8",
  ).replace(/\{\{(\w+)\}\}/gu, (_, name) => ids[name]),
);
const origin = "https://bridge.test";

function reply({ status, body, headers }) {
  return new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: headers ?? { "content-type": "application/json" },
  });
}
function eventStream({ status, headers, frames }) {
  const encoder = new TextEncoder();
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const frame of frames) controller.enqueue(encoder.encode(frame));
        controller.close();
      },
    }),
    { status, headers },
  );
}
function bridge() {
  const requests = [];
  const fetch = async (url, init) => {
    requests.push({ url, method: init.method });
    const path = url.slice(origin.length);
    if (init.method === "POST") return reply(contract.create);
    if (init.method === "DELETE") return reply(contract.delete);
    assert.equal(path, contract.create.body.events_path);
    return eventStream(contract.events);
  };
  return { fetch, requests };
}

test("the desktop consumer follows the bridge's own subscription life", async () => {
  const { fetch, requests } = bridge();
  const output = [];
  const owner = {};
  const handle = {
    id: "h",
    owner,
    opening: {
      session_id: ids.session_id,
      opening_generation: ids.opening_generation,
      greeting: { bridge_epoch: ids.bridge_epoch },
    },
    mint: { origin },
    streams: new Set(),
  };
  const events = createBridgeEventStreams({
    fetch,
    check: () => {},
    headers: () => ({}),
    send: (_owner, event) => output.push(event),
    device: (_handle, path, options) =>
      jsonRequest(fetch, `${origin}${path}`, options),
    handleFor: () => handle,
    retire: () => assert.fail("the contract never retires the handle"),
  });

  const { stream } = await events.subscribe(
    owner,
    "h",
    { kind: "application" },
    { requestId: "00000000-0000-4000-8000-000000000001" },
  );
  while (!output.some((event) => event.event === "reset_required"))
    await new Promise(setImmediate);

  assert.deepEqual(
    output.map((event) => [event.event, event.data]),
    [
      [
        "hello",
        {
          bridge_epoch: ids.bridge_epoch,
          durable: false,
          opening_generation: ids.opening_generation,
          recovery: "current_state_refresh",
          subscription_id: ids.subscription_id,
        },
      ],
      ["event", { bridge_epoch: ids.bridge_epoch, type: "probe.pushed" }],
      [
        "reset_required",
        { reason: "disconnected", recovery: "current_state_refresh" },
      ],
    ],
    "the heartbeat is not an event and the end asks for a refresh",
  );
  assert.equal(handle.lastSubscription, ids.subscription_id);
  await assert.rejects(events.unsubscribe(owner, stream), {
    code: "invalid_stream",
  });
  assert.deepEqual(
    requests.map(({ method }) => method),
    ["POST", "GET"],
  );
});

test("the desktop consumer deletes a live subscription with the bridge's 204", async () => {
  const { fetch, requests } = bridge();
  const handle = {
    id: "h",
    owner: {},
    opening: {
      session_id: ids.session_id,
      opening_generation: ids.opening_generation,
    },
    mint: { origin },
    streams: new Set(),
  };
  const events = createBridgeEventStreams({
    fetch,
    check: () => {},
    headers: () => ({}),
    send: () => {},
    device: (_handle, path, options) =>
      jsonRequest(fetch, `${origin}${path}`, options),
    handleFor: () => handle,
    retire: () => assert.fail("the contract never retires the handle"),
  });
  const { stream } = await events.subscribe(
    handle.owner,
    "h",
    { kind: "application" },
    { requestId: "00000000-0000-4000-8000-000000000002" },
  );

  await events.unsubscribe(handle.owner, stream);

  assert.deepEqual(requests.at(-1), {
    url: `${origin}/v1/subscriptions/${ids.subscription_id}`,
    method: "DELETE",
  });
});

for (const name of ["stream_required", "stream_replaced"]) {
  test(`the bridge's ${name} refusal reaches the caller as its domain error`, async () => {
    const value = await jsonRequest(
      async () => reply(contract[name]),
      `${origin}/v1/session-greetings`,
      { method: "POST", body: {}, apiEnvelope: true },
    );
    assert.equal(value.error_code, name);
    assert.equal(value.retryable, false);
  });
}
