import { describe, expect, it, vi } from "vitest";
import { openHttpSession } from "../src/core/httpSession.js";
import { createHttpRoutes } from "../src/core/httpRoutes.js";

const manifest = { version: 1, operations: [
  { method: "ping", operationId: "ping", verb: "GET", path: "/v1/health", pathParams: [], queryParams: [], body: "remaining" },
  { method: "tasks.update", operationId: "updateTask", verb: "PATCH", path: "/v1/tasks/{task_id}", pathParams: ["task_id"], queryParams: [], body: "remaining" },
], exclusions: [] };
const greeting = { api_version: "3.24.0", capabilities: ["bridge.epoch"], bridge_epoch: "epoch", journal_incarnation: "journal", ack_confirm_methods: [] };
const opening = { session_id: "session", opening_generation: "opening", grant_expires_at: new Date(Date.now() + 30000).toISOString(), greeting,
  transport: { resource_manifest_version: 1, operation_ids: ["ping", "updateTask"], terminal: "webrtc", durable_events: false } };
const make = (changes = {}) => {
  let consume;
  const endpoint = {
    identity: { deviceId: "device" }, open: vi.fn().mockResolvedValue(opening),
    subscribe: vi.fn(async (_opening, listener) => { consume = listener; await listener({ event: "hello", data: { durable: false, recovery: "current_state_refresh", bridge_epoch: "epoch", opening_generation: "opening" } }); return { binding: "stream", close: vi.fn() }; }),
    request: vi.fn().mockResolvedValue({ ok: true, result: {}, bridge_epoch: "epoch" }), close: vi.fn(), ...changes,
  };
  return { endpoint, event: (value) => consume(value) };
};
const open = (endpoint, options = {}) => openHttpSession({ endpoint, routes: createHttpRoutes(manifest), ...options });

describe("HTTP session adapter and fences", () => {
  it("requires a complete authenticated resource inventory before streaming", async () => {
    for (const transport of [undefined, { ...opening.transport, operation_ids: ["ping"] }, { ...opening.transport, resource_manifest_version: 2 }]) {
      const { endpoint } = make({ open: vi.fn().mockResolvedValue({ ...opening, transport }) });
      await expect(open(endpoint)).rejects.toMatchObject({ code: "unsupported" });
      expect(endpoint.subscribe).not.toHaveBeenCalled();
      expect(endpoint.close).toHaveBeenCalled();
    }
  });

  it("waits for the stream hello, then supplies the greeting and explicit resource calls", async () => {
    const { endpoint } = make();
    const session = await open(endpoint);
    expect(await session.rawCall("session.hello")).toEqual(greeting);
    await session.rawCall("tasks.update", { task_id: "task", body: "text" }, { priority: "background", write: { operation_id: "op", after: [] } });
    expect(endpoint.request).toHaveBeenCalledWith(opening, expect.objectContaining({ path: "/v1/tasks/task", body: { body: "text" } }), expect.objectContaining({ subscriptionId: "stream", priority: "background", write: { operation_id: "op", after: [] } }));
    session.close();
  });

  it("preserves typed refusals and write acknowledgments", async () => {
    const { endpoint } = make({ request: vi.fn().mockResolvedValueOnce({ ok: false, error: "precondition", error_code: "stale_body", retryable: false, details: { current: "hash" }, bridge_epoch: "epoch" })
      .mockResolvedValueOnce({ ok: true, write: { phase: "ack" }, result: { changed: true }, bridge_epoch: "epoch" }) });
    const session = await open(endpoint);
    await expect(session.rawCall("tasks.update", { task_id: "task" })).rejects.toMatchObject({ error_code: "stale_body", details: { current: "hash" }, uncertain: false });
    const writes = [];
    session.onWriteFrame((frame) => writes.push(frame));
    expect(await session.rawCall("tasks.update", { task_id: "task" }, { write: { operation_id: "op" } })).toMatchObject({ write: { phase: "ack" } });
    expect(writes).toHaveLength(1);
    session.close();
  });

  it("ends on transient stream reset and rejects old replies without publishing", async () => {
    let resolve;
    const { endpoint, event } = make({ request: vi.fn(() => new Promise((done) => { resolve = done; })) });
    const lost = vi.fn(); const pushed = vi.fn();
    const session = await open(endpoint, { onLost: lost, onPush: pushed });
    const pending = session.rawCall("tasks.update", { task_id: "task" });
    await event({ event: "reset_required", data: {} });
    resolve({ ok: true, result: { obsolete: true }, bridge_epoch: "epoch" });
    await expect(pending).rejects.toMatchObject({ uncertain: true });
    expect(lost).toHaveBeenCalledTimes(1);
    await event({ event: "event", data: { type: "changes", bridge_epoch: "epoch" } });
    expect(pushed).not.toHaveBeenCalled();
  });

  it("rejects wrong epoch push/reply and wrong opening hello", async () => {
    const { endpoint, event } = make({ request: vi.fn().mockResolvedValue({ ok: true, result: {}, bridge_epoch: "other" }) });
    const pushed = vi.fn(); const session = await open(endpoint, { onPush: pushed });
    await expect(session.rawCall("ping")).rejects.toThrow();
    await event({ event: "event", data: { type: "changes", bridge_epoch: "other" } });
    expect(pushed).not.toHaveBeenCalled();
  });

  it("does not send after captured scope is cancelled or account replaced", async () => {
    let active = true;
    const { endpoint } = make(); const session = await open(endpoint, { isCurrent: () => active });
    active = false;
    await expect(session.rawCall("tasks.update", { task_id: "task" })).rejects.toMatchObject({ uncertain: false });
    expect(endpoint.request).not.toHaveBeenCalled();
    session.close();
  });
});
