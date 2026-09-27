import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

const timing = { reopenDelaysMs: [0, 1, 2, 3], restMs: 20, openTimeoutMs: 200, giveUpAfterMs: 60000 };
const count = { deviceId: "", entityId: "", kind: "review-count" };
const lost = () => new DOMException("Connection to Indexed Database server lost", "UnknownError");
let cache;

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  cache.setCacheRecoveryTiming(timing);
  vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => vi.restoreAllMocks());

it("keeps counter uniqueness across module contexts when allocations retry", async () => {
  await cache.takeCachedCount(count);
  vi.resetModules();
  const other = await import("../src/core/localCache.js");
  other.setCacheRecoveryTiming(timing);
  await other.readCached(count);
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < 3) throw lost();
    return transaction.apply(this, args);
  });
  const results = await Promise.all([cache.takeCachedCount(count), other.takeCachedCount(count)]);
  expect(results.sort()).toEqual([2, 3]);
});

it("does not answer undefined for a count while transient transactions fail across a round", async () => {
  await cache.takeCachedCount(count);
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < 4) throw lost();
    return transaction.apply(this, args);
  });
  expect(await cache.takeCachedCount(count)).toBe(2);
});

it("keeps counter calls asked in this tab in order across an exhausted round", async () => {
  cache.setCacheRecoveryTiming({ ...timing, restMs: 200 });
  await cache.takeCachedCount(count);
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < timing.reopenDelaysMs.length) throw lost();
    return transaction.apply(this, args);
  });

  const first = cache.takeCachedCount(count);
  await vi.waitFor(() => expect(failures).toBe(timing.reopenDelaysMs.length), { interval: 1 });
  const second = cache.takeCachedCount(count);
  expect(await Promise.all([first, second])).toEqual([2, 3]);
});

it("does not allocate duplicate task read orders across tabs on transient count failure", async () => {
  const first = await import("../src/core/taskReadOrder.js");
  await first.nextTaskRead();
  vi.resetModules();
  const other = await import("../src/core/localCache.js");
  other.setCacheRecoveryTiming(timing);
  const second = await import("../src/core/taskReadOrder.js");
  await second.nextTaskRead();
  vi.spyOn(Date, "now").mockReturnValue(1800000000000);
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < 8) throw lost();
    return transaction.apply(this, args);
  });
  const results = await Promise.all([first.nextTaskRead(), second.nextTaskRead()]);
  expect(new Set(results).size).toBe(2);
});

it("still lets a plain write settle after one exhausted transient round", async () => {
  await cache.writeCached(count, "old");
  const transaction = IDBDatabase.prototype.transaction;
  let failures = 0;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    if (failures++ < timing.reopenDelaysMs.length) throw lost();
    return transaction.apply(this, args);
  });

  await cache.writeCached(count, "new");
  expect(failures).toBe(timing.reopenDelaysMs.length);
  expect((await cache.readCached(count))?.value).toBe("old");
});

it("refuses a local read order when the shared counter write is rejected", async () => {
  const order = await import("../src/core/taskReadOrder.js");
  const first = await order.nextTaskRead();
  const put = IDBObjectStore.prototype.put;
  const refusing = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (value, key) {
    if (String(key).includes("tracker-task-read-count"))
      throw new DOMException("The quota has been exceeded", "QuotaExceededError");
    return put.call(this, value, key);
  });

  await expect(order.nextTaskRead()).rejects.toMatchObject({ name: "QuotaExceededError" });
  refusing.mockRestore();
  expect(await order.nextTaskRead()).toBeGreaterThan(first);
});

it("uses a local order only when IndexedDB is genuinely absent", async () => {
  delete globalThis.indexedDB;
  vi.resetModules();
  const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
  const first = await nextTaskRead();
  expect(await nextTaskRead()).toBeGreaterThan(first);
});
