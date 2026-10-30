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

it("applies duplicate-address put then delete and delete then put in reducer order", async () => {
  const address = addresses[0];
  expect(await cache.mergeCachedRecordsTogether([address, address], () => [{ count: 1 }, cache.DELETE_CACHED_RECORD])).toBe(true);
  expect(await cache.readCached(address)).toBeUndefined();
  await cache.writeCached(address, { count: 0 });
  expect(await cache.mergeCachedRecordsTogether([address, address], () => [cache.DELETE_CACHED_RECORD, { count: 2 }])).toBe(true);
  expect((await cache.readCached(address)).value).toEqual({ count: 2 });
});

it("preserves duplicate-key ordering for repeated deletes and null interleavings from present and absent baselines", async () => {
  const drop = cache.DELETE_CACHED_RECORD;
  const cases = [
    { steps: [{ count: 1 }, drop], final: undefined, changedAbsent: true },
    { steps: [drop, { count: 2 }], final: { count: 2 }, changedAbsent: true },
    { steps: [drop, drop], final: undefined, changedAbsent: false },
    { steps: [{ count: 1 }, null, drop, null], final: undefined, changedAbsent: true },
    { steps: [null, drop, null, { count: 2 }], final: { count: 2 }, changedAbsent: true },
    { steps: [drop, null, drop], final: undefined, changedAbsent: false },
  ];
  for (const present of [false, true]) for (const [index, sample] of cases.entries()) {
    const address = { ...addresses[0], sub: `matrix-${present}-${index}` };
    if (present) await cache.writeCached(address, { count: 0 });
    const changed = await cache.mergeCachedRecordsTogether(sample.steps.map(() => address), () => sample.steps);
    expect(changed).toBe(present || sample.changedAbsent);
    expect((await cache.readCached(address))?.value).toEqual(sample.final);
  }
});
