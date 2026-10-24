import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const message = (sequence) => ({ type: "message", data: { sequence, body: `message ${sequence}` } });
const windowAt = (...items) => ({ items, deliveredSequence: items.at(-1)?.data.sequence || 0, olderItemsRemain: true, activityDigests: [] });
const address = { deviceId: "dev", entityId: "run", kind: "thread", sub: "conversation" };

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it("drops a retained prefix even when catch-up returns no newer rows, without repair", async () => {
  const cache = await import("../src/core/localCache.js");
  const { syncThreadWindow } = await import("../src/core/threadSync.js");
  await cache.writeCached(address, windowAt(message(1), message(8)));
  const call = vi.fn().mockResolvedValue({ items: [], history_start_sequence: 8, thread_last_sequence: 8, thread_total: 1, has_more: false });
  await syncThreadWindow({ ...address, conversationId: address.sub, call });
  const held = (await cache.readCached(address)).value;
  expect(held.items.map((item) => item.data.sequence)).toEqual([8]);
  expect(held.historyStartSequence).toBe(8);
  expect(held.olderItemsRemain).toBe(false);
  expect(call).toHaveBeenCalledTimes(1);
});

it("empties an expired conversation and advances its catch-up cursor past the removed history", async () => {
  const { threadWindow } = await import("../src/core/threadSync.js");
  const held = threadWindow(windowAt(message(1), message(8)), { items: [], history_start_sequence: 10, thread_last_sequence: 9, thread_total: 0, has_more: false });
  expect(held.items).toEqual([]);
  expect(held.deliveredSequence).toBe(9);
  expect(held.olderItemsRemain).toBe(false);
});

it("does not resurrect pruned messages or digests from an older in-flight page", async () => {
  const { threadWindow, repairedThreadWindow } = await import("../src/core/threadSync.js");
  const held = { ...windowAt(message(8)), historyStartSequence: 8 };
  const stale = { items: [message(1)], activity_digests: [{ from_sequence: 2, through_sequence: 7 }], history_start_sequence: 1, has_more: true };
  const merged = threadWindow(held, stale);
  expect(merged.items.map((item) => item.data.sequence)).toEqual([8]);
  expect(merged.activityDigests).toEqual([]);
  expect(merged.historyStartSequence).toBe(8);
  const repaired = repairedThreadWindow({ ...windowAt(message(1), message(8)), repairThrough: 10 }, { items: [], history_start_sequence: 8, thread_last_sequence: 10 });
  expect(repaired.items.map((item) => item.data.sequence)).toEqual([8]);
});

it("applies an advancing boundary from an older-page read and fences a stale widening", async () => {
  const cache = await import("../src/core/localCache.js");
  const { widenCachedThread } = await import("../src/core/agentRailContext.js");
  await cache.writeCached(address, windowAt(message(1), message(8)));
  await widenCachedThread(address, { items: [], history_start_sequence: 8, has_more: false }, 1);
  await widenCachedThread(address, { items: [message(2)], history_start_sequence: 1, has_more: true }, 8);
  const held = (await cache.readCached(address)).value;
  expect(held.items.map((item) => item.data.sequence)).toEqual([8]);
  expect(held.olderItemsRemain).toBe(false);
});


it("retains the floor when a stale newest page replaces a window", async () => {
  const { threadWindow } = await import("../src/core/threadSync.js");
  const held = { ...windowAt(message(8)), historyStartSequence: 8 };
  const merged = threadWindow(held, { items: [message(1)], has_more: true }, { newest: true });
  expect(merged.items).toEqual([message(8)]);
  expect(merged.historyStartSequence).toBe(8);
});

it("keeps local pending messages but deletes old rows even when they were recently updated", async () => {
  const { trimThreadHistory } = await import("../src/core/threadRetention.js");
  const pending = { data: { provisional: true, operation_id: "pending", body: "new draft" } };
  const old = { ...message(1), data: { ...message(1).data, updated_sequence: 20 } };
  const merged = trimThreadHistory(windowAt(old, pending, message(8)), { history_start_sequence: 8 });
  expect(merged.items).toEqual([pending, message(8)]);
  expect(trimThreadHistory({ ...windowAt(message(1)), thread_id: "new" }, { thread_id: "old", history_start_sequence: 8 }).items).toEqual([message(1)]);
});

it("does not repair the deliberately removed prefix when catch-up crosses the retention floor", async () => {
  const cache = await import("../src/core/localCache.js");
  const { syncThreadWindow } = await import("../src/core/threadSync.js");
  await cache.writeCached(address, windowAt(message(1)));
  const call = vi.fn().mockResolvedValue({ items: [message(8)], history_start_sequence: 8, thread_last_sequence: 8, has_more: false });
  await syncThreadWindow({ ...address, conversationId: address.sub, call });
  expect((await cache.readCached(address)).value.items).toEqual([message(8)]);
  expect(call).toHaveBeenCalledTimes(1);
});
