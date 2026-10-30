import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
let cache;
const addresses = ["one", "two", "three"].map((sub) => ({ deviceId: "delete-test", entityId: "project", kind: "atomic-delete", sub }));
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
});

it("commits a joint delete and put while retaining null no-op records and announcing only after commit", async () => {
  for (const [index, address] of addresses.entries()) await cache.writeCached(address, { count: index });
  const before = await cache.readCached(addresses[2]), announced = [], completedAtAnnouncement = [];
  let completed = false;
  const { IDBDatabase } = await import("fake-indexeddb"), transaction = IDBDatabase.prototype.transaction;
  const spy = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function(...args) {
    const tx = transaction.apply(this, args);
    if (args[1] === "readwrite") tx.addEventListener("complete", () => { completed = true; });
    return tx;
  });
  const stops = addresses.map((address, index) => cache.subscribeCache(address, () => {
    announced.push(index); completedAtAnnouncement.push(completed);
  }));
  expect(await cache.mergeCachedRecordsTogether(addresses, () => [cache.DELETE_CACHED_RECORD, { count: 20 }, null])).toBe(true);
  stops.forEach((stop) => stop()); spy.mockRestore();
  expect(completedAtAnnouncement).toEqual([true, true]);
  expect(await cache.readCached(addresses[0])).toBeUndefined();
  expect((await cache.readCached(addresses[1])).value).toEqual({ count: 20 });
  expect(await cache.readCached(addresses[2])).toEqual(before);
  expect(announced.sort()).toEqual([0, 1]);
});

it("treats deletion of an absent record as a no-op without a success announcement", async () => {
  const announced = vi.fn(), stop = cache.subscribeCache(addresses[0], announced);
  expect(await cache.mergeCachedRecordsTogether([addresses[0]], () => [cache.DELETE_CACHED_RECORD])).toBe(false);
  stop(); expect(announced).not.toHaveBeenCalled(); expect(await cache.readCached(addresses[0])).toBeUndefined();
});

it("rolls a deletion and companion writes back without announcing success when a later put hits quota", async () => {
  for (const [index, address] of addresses.entries()) await cache.writeCached(address, { count: index });
  const before = await cache.readCachedMany(addresses), announced = vi.fn();
  const stops = addresses.map((address) => cache.subscribeCache(address, announced));
  const { IDBObjectStore } = await import("fake-indexeddb"), put = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(record, key) {
    if (record?.value?.count === 20) throw new DOMException("Quota", "QuotaExceededError");
    return put.call(this, record, key);
  });
  expect(await cache.mergeCachedRecordsTogether(addresses, () => [{ count: 10 }, cache.DELETE_CACHED_RECORD, { count: 20 }])).toBe(false);
  spy.mockRestore(); stops.forEach((stop) => stop());
  expect(await cache.readCachedMany(addresses)).toEqual(before); expect(announced).not.toHaveBeenCalled();
});
