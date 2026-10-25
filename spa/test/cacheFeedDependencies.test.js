// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it.each(["IndexedDB", "memory"])("%s feed updates supply canonical dependency records and retain captured-write checks", async (backend) => {
  const cache = backend === "memory" ? await import("./memoryCache.js") : await import("../src/core/localCache.js");
  const feed = { deviceId: "dev-1", entityId: "", kind: "feed", sub: "" };
  const row = { ...feed, entityId: "run-1", kind: "row" };
  const thread = { ...row, kind: "thread", sub: "agent-1" };
  const missing = { ...thread, sub: "not-stored" };
  await cache.writeCached(row, { label: "before" });
  await cache.writeCached(thread, { thread_id: "current-generation" });
  const captured = await cache.captureCachedRecord(row);
  const update = vi.fn((_held, _record, dependencies) => {
    expect(dependencies).toEqual([
      expect.objectContaining({ value: { thread_id: "current-generation" } }), undefined,
    ]);
    return { items: [{ run_id: "run-1", thread_id: dependencies[0].value.thread_id }] };
  });
  const options = { readAddresses: [thread, missing], unchanged: [{ address: row, written: captured.write }] };
  expect(await cache.updateCachedFeed(feed, update, options)).toBe(true);
  expect((await cache.readCached(feed)).value.items[0].thread_id).toBe("current-generation");
  expect(await cache.readCached(missing)).toBeUndefined();
  await cache.writeCached(row, { label: "after" });
  expect(await cache.updateCachedFeed(feed, update, options)).toBe(false);
  expect(update).toHaveBeenCalledTimes(1);
});
