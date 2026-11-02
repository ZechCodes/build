import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { createBrowserHttpEndpoint } from "../src/core/httpBrowserEndpoint.js";
import { readTransientEvents } from "../src/core/httpSse.js";

// The bridge writes this fixture from its own responses
// (bridge/src/http/runtime/tests/subscriptions/contract.rs).
const ids = {
  session_id: "c0a80001-0000-4000-8000-000000000001",
  subscription_id: "c0a80001-0000-4000-8000-000000000002",
  opening_generation: "c0a80001-0000-4000-8000-000000000003",
  bridge_epoch: "c0a80001-0000-4000-8000-000000000004",
};
const contract = JSON.parse(readFileSync(new URL("../../fixtures/api/http/subscription-contract.json", import.meta.url), "utf8")
  .replace(/\{\{(\w+)\}\}/gu, (_, name) => ids[name]));
const origin = "https://bridge.example";
const opening = { session_id: ids.session_id, opening_generation: ids.opening_generation, grant_expires_at: new Date(Date.now() + 30000).toISOString(), greeting: { bridge_epoch: ids.bridge_epoch } };
const grant = () => ({ identity: { origin, deviceId: "device" }, token: () => "device-token", expiresAt: () => Date.now() + 300000, renew: vi.fn(), close: vi.fn() });
const reply = ({ status, body }) => new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const eventStream = ({ status, headers, frames }, { end }) => new Response(new ReadableStream({
  start(controller) {
    for (const frame of frames) controller.enqueue(new TextEncoder().encode(frame));
    if (end) controller.close();
  },
}), { status, headers });
const helloAndPush = [
  { event: "hello", data: { bridge_epoch: ids.bridge_epoch, durable: false, opening_generation: ids.opening_generation, recovery: "current_state_refresh", subscription_id: ids.subscription_id } },
  { event: "event", data: { bridge_epoch: ids.bridge_epoch, type: "probe.pushed" } },
];

describe("the bridge's own subscription contract", () => {
  it("reads the bridge's event stream as hello and a push, with the heartbeat as activity only", async () => {
    const events = [];
    let activity = 0;
    await readTransientEvents(eventStream(contract.events, { end: true }), (event) => events.push(event), () => { activity += 1; });
    expect(events).toEqual(helloAndPush);
    expect(activity).toBe(contract.events.frames.length);
  });

  it("subscribes, streams and deletes through the bridge's own replies", async () => {
    const fetchImpl = vi.fn(async (url, { method }) => {
      if (method === "POST" && url.endsWith("/v1/sessions")) return reply({ status: 201, body: opening });
      if (method === "POST") return reply(contract.create);
      if (method === "DELETE") return reply(contract.delete);
      return eventStream(contract.events, { end: false });
    });
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    await endpoint.open(">=3 <4");
    const consumed = [];

    const stream = await endpoint.subscribe(opening, (event) => consumed.push(event));
    await vi.waitFor(() => expect(consumed).toHaveLength(2));
    stream.close();

    expect(stream.binding).toBe(ids.subscription_id);
    expect(consumed).toEqual(helloAndPush);
    expect(fetchImpl.mock.calls.map(([url, { method }]) => [method, url.slice(origin.length)])).toEqual([
      ["POST", "/v1/sessions"],
      ["POST", "/v1/subscriptions"],
      ["GET", contract.create.body.events_path],
      ["DELETE", `/v1/subscriptions/${ids.subscription_id}`],
    ]);
    endpoint.close();
  });

  it.each(["stream_required", "stream_replaced"])("returns the bridge's %s refusal as its domain error", async (name) => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(reply({ status: 201, body: opening })).mockResolvedValueOnce(reply(contract[name]))
      .mockResolvedValue(new Response(null, { status: 204 }));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    const held = await endpoint.open(">=3 <4");

    const refusal = await endpoint.request(held, { path: "/v1/session-greetings", method: "POST", body: {} }, {});

    expect(refusal).toMatchObject({ error_code: name, retryable: false });
    endpoint.close();
  });
});
