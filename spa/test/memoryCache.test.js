import { beforeEach, expect, it, vi } from "vitest";
import * as cache from "./memoryCache.js";

const address = (entityId, kind = "status", deviceId = "device") => ({ deviceId, entityId, kind, sub: "" });
beforeEach(() => cache.resetMemoryCache());

it("expires only selected stale keys, preserving UI, exact-cutoff and explicitly kept records", async () => {
  vi.spyOn(Date, "now").mockReturnValue(10);
  for (const at of [address("a"), address("ab"), address("a", "ui-tab"), address("a", "row"), address("a", "status", "other")]) await cache.writeCached(at, {});
  vi.spyOn(Date, "now").mockReturnValue(20);
  await cache.writeCached(address("a", "tree"), {});
  vi.restoreAllMocks();
  const heard = vi.fn();
  cache.subscribeCache({ deviceId: "device" }, heard);
  const result = await cache.expireCachedPrefixes([{ deviceId: "device", entityId: "a" }], 20, { keep: (at) => at.kind === "row" });
  expect(result.dropped).toEqual([address("a")]);
  expect(result.kept).toEqual(expect.arrayContaining([address("a", "ui-tab"), address("a", "row"), address("a", "tree")]));
  expect(await cache.readCached(address("ab"))).toBeDefined();
  expect(await cache.readCached(address("a", "status", "other"))).toBeDefined();
  expect(heard).toHaveBeenCalledExactlyOnceWith(address("a"));
});

it("skips expiry without announcing when its activity predicate refuses", async () => {
  await cache.writeCached(address("a"), {});
  const heard = vi.fn();
  cache.subscribeCache({}, heard);
  expect(await cache.expireCachedPrefixes([{ deviceId: "device" }], Infinity, { still: () => false })).toEqual({ dropped: [], kept: [] });
  expect(await cache.readCached(address("a"))).toBeDefined();
  expect(heard).not.toHaveBeenCalled();
});

it("admits sibling read repairs but fences ordinary writes and silent no-ops", async () => {
  const at = address("a", "row");
  await cache.writeCached(at, { cursor: 10 });
  const before = await cache.captureCachedRecord(at);
  const merge = vi.fn((held, original) => ({ cursor: held.cursor + (original ? 1 : 2) }));
  expect(await cache.mergeCachedReadState(at, before, merge)).toBe(true);
  expect(await cache.mergeCachedReadState(at, before, merge)).toBe(true);
  expect(merge.mock.calls.map((call) => call[1])).toEqual([true, false]);
  expect((await cache.readCached(at)).value.cursor).toBe(13);
  await cache.writeCached(at, { cursor: 13 });
  const heard = vi.fn();
  cache.subscribeCache(at, heard);
  expect(await cache.mergeCachedReadState(at, before, merge)).toBe(false);
  const current = await cache.captureCachedRecord(at);
  expect(cache.isCachedWrite(current, cache.cachedWriteOf(current))).toBe(true);
  expect(cache.isCachedWrite(undefined, undefined)).toBe(false);
  expect(await cache.mergeCachedReadState(at, current, () => null)).toBe(false);
  expect(heard).not.toHaveBeenCalled();
});

it("keeps feed repair lineage until an ordinary update and enforces dependency writes", async () => {
  const at = address("", "feed");
  const dependency = address("a", "row");
  await cache.writeCached(at, { cursor: 10 });
  await cache.writeCached(dependency, {});
  const before = await cache.captureCachedRecord(at);
  const written = cache.cachedWriteOf(await cache.readCached(dependency));
  expect(await cache.updateCachedFeed(at, () => ({ cursor: 11 }), { readStateFrom: before })).toBe(true);
  expect(await cache.updateCachedFeed(at, () => ({ cursor: 12 }), { readStateFrom: before })).toBe(true);
  await cache.writeCached(dependency, {});
  expect(await cache.updateCachedFeed(at, () => ({ cursor: 13 }), { unchanged: [{ address: dependency, written }] })).toBe(false);
  await cache.updateCachedFeed(at, (held) => held);
  const heard = vi.fn();
  cache.subscribeCache(at, heard);
  expect(await cache.updateCachedFeed(at, () => ({ cursor: 14 }), { readStateFrom: before })).toBe(false);
  expect((await cache.readCached(at)).value.cursor).toBe(12);
  expect(heard).not.toHaveBeenCalled();
});

it("merges whole records together and publishes only after every write is committed", async () => {
  const one = address("a");
  const two = address("b");
  await cache.writeCached(one, { count: 1 });
  const before = await cache.readCached(one);
  const observed = [];
  cache.subscribeCache(one, () => observed.push(cache.readCached(two)));
  expect(await cache.mergeCachedRecordsTogether([one, two], ([held, absent]) => {
    expect(held).toEqual(before);
    expect(absent).toBeUndefined();
    return [{ count: held.value.count + 1 }, { count: 3 }];
  })).toBe(true);
  expect((await observed[0]).value.count).toBe(3);
  expect(await cache.mergeCachedRecordsTogether([one], () => [null])).toBe(false);
});
