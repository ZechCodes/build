import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBIndex, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, lifetime;
const NOW = 1_800_000_000_000;
const OLD = NOW - 100 * 60 * 60 * 1000;
const address = (entityId, kind = "status", deviceId = "device|1") => ({ deviceId, entityId, kind, sub: "" });
const writeAt = async (at, value = {}) => {
  vi.spyOn(Date, "now").mockReturnValue(OLD);
  await cache.writeCached(at, value);
  vi.restoreAllMocks();
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  cache.setCacheRecoveryTiming({ reopenDelaysMs: [0, 1, 2], restMs: 5 });
  lifetime = await import("../src/core/cacheLifetime.js");
});
afterEach(() => vi.restoreAllMocks());

it("expires Recent entities together using one stale-key read and bounded entity keys, without reading bodies", async () => {
  for (const at of [address("a|one"), address("b"), address("a|one", "row"), address("a|one-long"), address("a|one", "status", "other")]) await writeAt(at);
  vi.spyOn(Date, "now").mockReturnValue(NOW - lifetime.WORKSPACE_DATA_TTL_MS);
  await cache.writeCached(address("a|one", "tree"), { exactCutoff: true });
  vi.restoreAllMocks();
  const transactions = vi.spyOn(IDBDatabase.prototype, "transaction");
  const staleRead = vi.spyOn(IDBIndex.prototype, "getAllKeys");
  const entityRead = vi.spyOn(IDBObjectStore.prototype, "getAllKeys");
  const bodies = ["get", "getAll", "openCursor"].map((name) => vi.spyOn(IDBObjectStore.prototype, name));
  const cursor = vi.spyOn(IDBIndex.prototype, "openKeyCursor");
  const heard = [];
  cache.subscribeCache({ deviceId: "device|1" }, (at) => heard.push(at));
  const result = await lifetime.expireWorkspacesData("device|1", ["a|one", "b", "a|one"], NOW);
  expect(result.dropped).toEqual([address("a|one"), address("b")]);
  expect(result.kept).toEqual(expect.arrayContaining([address("a|one", "row"), address("a|one", "tree")]));
  expect(transactions.mock.calls.map((call) => call[1])).toEqual(["readwrite"]);
  expect(staleRead).toHaveBeenCalledTimes(1);
  expect(entityRead).toHaveBeenCalledTimes(2);
  expect(entityRead.mock.calls.every(([range]) => range instanceof IDBKeyRange && range.lower !== undefined && range.upper !== undefined)).toBe(true);
  for (const spy of [...bodies, cursor]) expect(spy).not.toHaveBeenCalled();
  expect(heard).toEqual(result.dropped);
  vi.restoreAllMocks();
  expect(await cache.readCached(address("a|one-long"))).toBeDefined();
  expect(await cache.readCached(address("a|one", "status", "other"))).toBeDefined();
  expect(await cache.readCached(address("a|one", "tree"))).toBeDefined();
});

it("checks activity after queued transaction reads, skipping every delete and announcement", async () => {
  await writeAt(address("a"));
  const heard = [];
  cache.subscribeCache({ deviceId: "device|1" }, (at) => heard.push(at));
  let active = true;
  const getKeys = IDBObjectStore.prototype.getAllKeys;
  vi.spyOn(IDBObjectStore.prototype, "getAllKeys").mockImplementation(function (...args) {
    const request = getKeys.apply(this, args);
    request.addEventListener("success", () => { active = false; });
    return request;
  });
  const result = await lifetime.expireWorkspacesData("device|1", ["a"], NOW, () => active);
  expect(result.dropped).toEqual([]);
  expect(heard).toEqual([]);
  expect(await cache.readCached(address("a"))).toBeDefined();
});

it("re-evaluates expiry activity after a transaction abort and announces only committed deletes", async () => {
  await writeAt(address("a"));
  const heard = [];
  cache.subscribeCache({ deviceId: "device|1" }, (at) => heard.push(at));
  const still = vi.fn().mockReturnValueOnce(true).mockReturnValue(false);
  const remove = IDBObjectStore.prototype.delete;
  let aborted = false;
  vi.spyOn(IDBObjectStore.prototype, "delete").mockImplementation(function (...args) {
    const request = remove.apply(this, args);
    if (!aborted) { aborted = true; this.transaction.abort(); }
    return request;
  });
  const result = await lifetime.expireWorkspacesData("device|1", ["a"], NOW, still);
  expect(still).toHaveBeenCalledTimes(2);
  expect(result.dropped).toEqual([]);
  expect(heard).toEqual([]);
  expect(await cache.readCached(address("a"))).toBeDefined();
});

it("keeps a freshly rewritten record before expiry's queued transaction executes", async () => {
  await writeAt(address("a"));
  vi.spyOn(Date, "now").mockReturnValue(NOW);
  const fresh = cache.writeCached(address("a"), { fresh: true });
  const expiry = lifetime.expireWorkspacesData("device|1", ["a"], NOW);
  await fresh;
  expect((await expiry).dropped).toEqual([]);
  expect((await cache.readCached(address("a"))).value).toEqual({ fresh: true });
});

it.each(["evictEntity", "deleteCached"])("retains the existing %s queued predicate, retry and silent skip guarantees", async (method) => {
  await writeAt(address("a"));
  const heard = [];
  cache.subscribeCache({ deviceId: "device|1" }, (at) => heard.push(at));
  const still = vi.fn().mockReturnValueOnce(true).mockReturnValue(false);
  const remove = IDBObjectStore.prototype.delete;
  let aborted = false;
  vi.spyOn(IDBObjectStore.prototype, "delete").mockImplementation(function (...args) {
    const request = remove.apply(this, args);
    if (!aborted) { aborted = true; this.transaction.abort(); }
    return request;
  });
  if (method === "evictEntity") await cache.evictEntity("device|1", "a", { still });
  else await cache.deleteCached([address("a")], { still });
  expect(still).toHaveBeenCalledTimes(2);
  expect(heard).toEqual([]);
  expect(await cache.readCached(address("a"))).toBeDefined();
});

it.each(["evictEntity", "deleteCached"])("checks %s's predicate only once its queued request executes", async (method) => {
  await writeAt(address("a"));
  const heard = [];
  cache.subscribeCache({ deviceId: "device|1" }, (at) => heard.push(at));
  let active = true;
  const getKey = IDBObjectStore.prototype.getKey;
  vi.spyOn(IDBObjectStore.prototype, "getKey").mockImplementation(function (...args) {
    const request = getKey.apply(this, args);
    request.addEventListener("success", () => { active = false; });
    return request;
  });
  if (method === "evictEntity") await cache.evictEntity("device|1", "a", { still: () => active });
  else await cache.deleteCached([address("a")], { still: () => active });
  expect(heard).toEqual([]);
  expect(await cache.readCached(address("a"))).toBeDefined();
});
