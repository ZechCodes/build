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
      await events.subscribe(owner, "h", { kind: "application" });
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
