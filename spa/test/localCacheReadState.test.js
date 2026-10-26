import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache;
const rowAddress = { deviceId: "device", entityId: "owner", kind: "row", sub: "" };
const feedAddress = { ...rowAddress, entityId: "", kind: "feed" };
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
});
afterEach(() => vi.restoreAllMocks());

it.each(["writeCached", "mergeCachedAtomically"])("an ordinary %s ends the row's repair lineage even when its value is identical", async (method) => {
  await cache.writeCached(rowAddress, { cursor: 10 });
  const before = await cache.captureCachedRecord(rowAddress);
  expect(await cache.mergeCachedReadState(rowAddress, before, () => ({ cursor: 11 }))).toBe(true);
  const repaired = await cache.captureCachedRecord(rowAddress);
  if (method === "writeCached") await cache.writeCached(rowAddress, repaired.value);
  else await cache.mergeCachedAtomically(rowAddress, () => repaired.value);
  const heard = vi.fn();
  cache.subscribeCache(rowAddress, heard);
  expect(await cache.mergeCachedReadState(rowAddress, before, () => ({ cursor: 12 }))).toBe(false);
  expect(await cache.mergeCachedReadState(rowAddress, repaired, () => ({ cursor: 12 }))).toBe(false);
  expect(heard).not.toHaveBeenCalled();
  expect((await cache.readCached(rowAddress)).value.cursor).toBe(11);
});

it("an ordinary feed update ends repair lineage while preserving row observations", async () => {
  await cache.writeCached(feedAddress, { items: [{ run_id: "owner", unread_count: 2 }] });
  const before = await cache.captureCachedRecord(feedAddress);
  expect(await cache.updateCachedFeed(feedAddress, (held) => ({ ...held, read: 11 }), { readStateFrom: before })).toBe(true);
  const repaired = await cache.captureCachedRecord(feedAddress);
  await cache.updateCachedFeed(feedAddress, (held) => held);
  const ordinary = await cache.readCached(feedAddress);
  const heard = vi.fn();
  cache.subscribeCache(feedAddress, heard);
  expect(await cache.updateCachedFeed(feedAddress, (held) => ({ ...held, read: 12 }), { readStateFrom: repaired })).toBe(false);
  expect(heard).not.toHaveBeenCalled();
  expect((await cache.readCached(feedAddress)).value).toEqual(ordinary.value);
  expect(ordinary.readStateBase).toBeUndefined();
});

it("does not carry read-repair lineage through eviction and owner recreation", async () => {
  await cache.writeCached(rowAddress, { thread_id: "old", cursor: 10 });
  const before = await cache.captureCachedRecord(rowAddress);
  await cache.mergeCachedReadState(rowAddress, before, () => ({ thread_id: "old", cursor: 11 }));
  await cache.evictEntity("device", "owner");
  await cache.writeCached(rowAddress, { thread_id: "new", cursor: 0 });
  expect(await cache.mergeCachedReadState(rowAddress, before, () => ({ thread_id: "old", cursor: 12 }))).toBe(false);
  expect((await cache.readCached(rowAddress)).value).toEqual({ thread_id: "new", cursor: 0 });
});
