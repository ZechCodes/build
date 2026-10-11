import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const address = { deviceId: "lean-device", entityId: "run-1", kind: "thread", sub: "agent-1" };
const message = (sequence, updated = sequence) => ({ type: "message", data: { sequence, updated_sequence: updated, role: "agent", body: "Hello" } });
const activity = (sequence) => ({ type: "event", data: { sequence, event: "tool_use", summary: "Read a.js" } });
const digest = { from_sequence: 2, through_sequence: 100, rows: 99, tool_calls: 99 };
let cache, sync, events;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/threadSync.js");
  events = await import("../src/core/changeEvents.js");
});

const greet = (capabilities) => events.greetBridge(async () => ({ api_version: "3.20.0", capabilities, push_events: false }), { deviceId: address.deviceId });
const read = (call) => sync.syncThreadWindow({ ...address, agentId: address.sub, call });

describe("lean thread page requests", () => {
  it.each([{ capabilities: undefined }, { capabilities: [] }, { capabilities: ["threads.leanActivity"] }])("gates cold, warm and repair pages on the advertised name: %j", async ({ capabilities }) => {
    if (capabilities) await greet(capabilities);
    const lean = capabilities?.includes("threads.leanActivity");
    const call = vi.fn()
      .mockResolvedValueOnce({ items: [message(1)], has_more: false, thread_last_sequence: 1 })
      .mockResolvedValueOnce({ items: [], has_more: false, thread_last_sequence: 2 })
      .mockResolvedValueOnce({ items: [message(1, 2)], has_more: false, thread_last_sequence: 2 });
    await read(call);
    await read(call);
    expect(call.mock.calls.map(([method, params]) => ({ method, params }))).toEqual([
      { method: "thread.page", params: { entity_id: "run-1", agent_id: "agent-1", limit: lean ? 23 : 100, ...(lean ? { activity: "newest" } : {}) } },
      { method: "thread.page", params: { entity_id: "run-1", agent_id: "agent-1", after_sequence: 1, newest: true, limit: 100, ...(lean ? { activity: "newest" } : {}) } },
      { method: "thread.page", params: { entity_id: "run-1", agent_id: "agent-1", after_sequence: 0, limit: 50, ...(lean ? { activity: "newest" } : {}) } },
    ]);
  });

  it("uses each device's current greeting, including a bridge downgrade", async () => {
    await greet(["threads.leanActivity"]);
    const call = vi.fn().mockResolvedValue({ items: [], has_more: false });
    await read(call);
    await sync.syncThreadWindow({ ...address, deviceId: "legacy-device", agentId: address.sub, call });
    await greet([]);
    await read(call);
    expect(call.mock.calls.map(([, params]) => params.activity)).toEqual(["newest", undefined, undefined]);
  });

  it("keeps held message context when has_more only describes omitted activity", async () => {
    await greet(["threads.leanActivity"]);
    await cache.writeCached(address, { items: [message(1)], deliveredSequence: 1, activityDigests: [] });
    const call = vi.fn().mockResolvedValue({ items: [activity(100)], activity_digests: [digest], has_more: true, thread_last_sequence: 100 });
    await read(call);
    const held = (await cache.readCached(address)).value;
    expect(held.items.map((item) => item.data.sequence)).toEqual([1, 100]);
    expect(held.activityDigests).toEqual([digest]);
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("remembers the requested mode across a re-greeting while its reply is in flight", async () => {
    await greet(["threads.leanActivity"]);
    await cache.writeCached(address, { items: [message(1)], deliveredSequence: 1, activityDigests: [] });
    const call = vi.fn(async () => {
      await greet([]);
      return { items: [activity(100)], activity_digests: [digest], has_more: true, thread_last_sequence: 100 };
    });
    await read(call);
    expect((await cache.readCached(address)).value.items.map((item) => item.data.sequence)).toEqual([1, 100]);
  });

  it("retains context when omitted activity follows an unshipped update counter", async () => {
    await greet(["threads.leanActivity"]);
    await cache.writeCached(address, { items: [message(1, 2)], deliveredSequence: 2, activityDigests: [] });
    const call = vi.fn().mockResolvedValueOnce({
      items: [activity(100)], activity_digests: [{ ...digest, from_sequence: 4, rows: 97 }], has_more: true, thread_last_sequence: 100,
    }).mockResolvedValue({ items: [message(1, 3), activity(100)], thread_last_sequence: 100 });
    await read(call);
    expect((await cache.readCached(address)).value.items.map((item) => item.data.sequence)).toEqual([1, 100]);
    expect((await cache.readCached(address)).value.items[0].data.updated_sequence).toBe(3);
  });

  it("replaces a stale window when the lean message budget skips real message context", async () => {
    await greet(["threads.leanActivity"]);
    await cache.writeCached(address, { items: [message(1)], deliveredSequence: 1, activityDigests: [] });
    await read(vi.fn().mockResolvedValue({ items: [message(200), activity(300)], activity_digests: [{ ...digest, from_sequence: 201, through_sequence: 300 }], has_more: true, thread_last_sequence: 300 }));
    expect((await cache.readCached(address)).value.items.map((item) => item.data.sequence)).toEqual([200, 300]);
  });

  it.each([{ refusal: false }, { refusal: true }, { refusal: false, provisional: true }])("repairs an omitted held activity item before paying its debt: %j", async ({ refusal, provisional }) => {
    await greet(["threads.leanActivity"]);
    const callItem = activity(2);
    const thought = { type: "event", data: { sequence: 3, event: "reasoning", summary: "Thinking" } };
    const queued = provisional ? Array.from({ length: 51 }, (_, index) => ({
      type: "message", data: { provisional: true, operation_id: `queued-${index}`, role: "user", body: "Queued" },
    })) : [];
    await cache.writeCached(address, { items: [message(1), callItem, thought, ...queued], deliveredSequence: 3, activityDigests: [] });
    const call = vi.fn(async (method, params) => {
      if (method === "thread.activity") {
        if (refusal) throw new Error("unavailable");
        return { items: [{ ...callItem, data: { ...callItem.data, updated_sequence: 4, outcome: "ok" } }] };
      }
      return params.newest
        ? { items: [], has_more: false, thread_last_sequence: 4 }
        : { items: [thought], has_more: false, thread_last_sequence: 4, activity_digests: [{ from_sequence: 2, through_sequence: 3, rows: 2, tool_calls: 1 }] };
    });
    await sync.syncThreadWindow({ ...address, agentId: address.sub, priority: "background", call });
    expect(call).toHaveBeenCalledWith("thread.activity", { entity_id: "run-1", agent_id: "agent-1", from_sequence: 2, through_sequence: 2, limit: 1 }, { priority: "background" });
    const held = (await cache.readCached(address)).value;
    expect(held.items.find((item) => item.data.sequence === 2).data.outcome).toBe(refusal ? undefined : "ok");
    expect(held.repairThrough).toBe(refusal ? 4 : undefined);
  });
});
