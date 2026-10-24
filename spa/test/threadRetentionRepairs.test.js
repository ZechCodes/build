import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const address = { deviceId: "retained", entityId: "run", kind: "thread", sub: "conversation" };
const message = (sequence, updated = sequence) => ({ type: "message", data: { sequence, updated_sequence: updated, body: `copy ${updated}` } });
const windowAt = (...items) => ({ thread_id: "thread", items, deliveredSequence: items.at(-1)?.data.sequence || 0 });
let cache, sync;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/threadSync.js");
});

it.each([false, true])("propagates an omitted exact/activity floor and skips removed held representatives (lean=%s)", async (lean) => {
  if (lean) {
    const events = await import("../src/core/changeEvents.js");
    await events.greetBridge(async () => ({ api_version: "3.25.0", capabilities: ["threads.leanActivity"], push_events: false }), { deviceId: address.deviceId });
  }
  const activity = (sequence) => ({ type: "event", data: { sequence, event: "tool_use" } });
  const old = lean ? [activity(100), activity(120)] : [message(100), message(120)];
  await cache.writeCached(address, windowAt(message(1), ...old, message(200)));
  const requested = [];
  const call = vi.fn(async (method, params) => {
    if (params.newest) return { thread_id: "thread", items: [], thread_last_sequence: 203 };
    if (params.limit === 50) return { thread_id: "thread", items: [message(1)], has_more: true, thread_last_sequence: 203,
      activity_digests: lean ? [{ from_sequence: 100, through_sequence: 120, rows: 2 }] : [] };
    const sequence = method === "thread.activity" ? params.from_sequence : params.after_sequence + 1;
    requested.push(sequence);
    return { thread_id: "thread", items: sequence < 150 ? [message(200)] : [message(200, 203)],
      history_start_sequence: 150, thread_last_sequence: 999, has_more: false };
  });
  await sync.syncThreadWindow({ ...address, conversationId: address.sub, call });
  const held = (await cache.readCached(address)).value;
  expect(held.historyStartSequence).toBe(150);
  expect(held.items).toEqual([message(200, 203)]);
  expect(held.repairThrough).toBeUndefined();
  expect(held.deliveredSequence).toBe(203);
  expect(requested).toEqual([100, 200]);
});

it("uses retained held rows to decide capped repair coverage", () => {
  const held = { ...windowAt(message(1), message(200)), repairThrough: 203 };
  const repaired = sync.repairedThreadWindow(held, { thread_id: "thread", items: [message(200, 203)], history_start_sequence: 150, has_more: true, thread_last_sequence: 203 });
  expect(repaired.items).toEqual([message(200, 203)]);
  expect(repaired.repairThrough).toBeUndefined();
});

it("clears obsolete repair debt when a floor empties the retained window", () => {
  const held = { ...windowAt(message(1), message(8)), repairThrough: 12 };
  const trimmed = sync.threadWindow(held, { thread_id: "thread", items: [], history_start_sequence: 13 });
  expect(trimmed.items).toEqual([]);
  expect(trimmed.repairThrough).toBeUndefined();
  expect(trimmed.deliveredSequence).toBe(12);
});

it("keeps the current floor when a stale capped repair arrives", () => {
  const held = { ...windowAt(message(200)), historyStartSequence: 150, repairThrough: 203 };
  const repaired = sync.repairedThreadWindow(held, { thread_id: "thread", items: [message(1), message(200, 203)], history_start_sequence: 1, has_more: true, thread_last_sequence: 203 });
  expect(repaired.historyStartSequence).toBe(150);
  expect(repaired.items).toEqual([message(200, 203)]);
  expect(repaired.repairThrough).toBeUndefined();
});

it("ignores an old generation's exact floor after a concurrent reset", async () => {
  await cache.writeCached(address, windowAt(message(1), message(100)));
  const call = vi.fn(async (_method, params) => {
    if (params.newest) return { thread_id: "thread", items: [], thread_last_sequence: 103 };
    if (params.limit === 50) return { thread_id: "thread", items: [message(1)], has_more: true, thread_last_sequence: 103 };
    await cache.writeCached(address, { thread_id: "replacement", items: [message(1)], historyStartSequence: 1, deliveredSequence: 1 });
    return { thread_id: "thread", items: [], history_start_sequence: 200, thread_last_sequence: 199 };
  });
  await sync.syncThreadWindow({ ...address, conversationId: address.sub, call });
  const held = (await cache.readCached(address)).value;
  expect(held.thread_id).toBe("replacement");
  expect(held.historyStartSequence).toBe(1);
  expect(held.items).toEqual([message(1)]);
});
