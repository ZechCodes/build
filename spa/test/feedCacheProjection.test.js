// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, rows, createFeedCacheReader;
const deviceId = "projection-device";
const rowAddress = (entityId) => ({ deviceId, entityId, kind: "row" });
const row = (id, extra = {}) => ({ entity_id: id, project_id: "p1", kind: "branch", branch: id, ...extra });
const rowReads = (spy) => spy.mock.calls.filter(([key]) => typeof key === "string" && key.split("|")[2] === "row").length;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  rows = await import("../src/core/cachedRows.js");
  ({ createFeedCacheReader } = await import("../src/core/feedCacheReader.js"));
  await Promise.all(Array.from({ length: 128 }, (_, index) => cache.writeCached(rowAddress(`r${index}`), row(`r${index}`))));
});
afterEach(() => vi.restoreAllMocks());

it("shares the initial row snapshot between concurrent route and feed readers", async () => {
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  const reader = createFeedCacheReader(deviceId);
  const [first, second, feed] = await Promise.all([
    rows.cachedFeedView(deviceId, { projectRosters: false }),
    rows.cachedFeedView(deviceId), reader.refresh(),
  ]);
  expect(first.items).toHaveLength(128);
  expect(second.items).toEqual(feed.view.items);
  expect(rowReads(gets)).toBe(128);
});

it("rereads only changed addresses across repeated route and feed lookups", async () => {
  const reader = createFeedCacheReader(deviceId);
  await reader.refresh();
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  await cache.writeCached({ deviceId, entityId: "other", kind: "thread" }, { messages: [] });
  await rows.cachedFeedView(deviceId);
  expect(rowReads(gets)).toBe(0);
  await cache.writeCached(rowAddress("r4"), row("r4", { muted: true }));
  gets.mockClear();
  const [route, feed] = await Promise.all([
    rows.cachedFeedView(deviceId, { projectRosters: false }), reader.refresh([rowAddress("r4")]),
  ]);
  expect(route.items.find((item) => item.entity_id === "r4").muted).toBe(true);
  expect(feed.view.items).toEqual(route.items);
  expect(rowReads(gets)).toBe(1);
});

it("invalidates the shared snapshot after deletion, entity eviction and reset", async () => {
  const reader = createFeedCacheReader(deviceId);
  await reader.refresh();
  await cache.deleteCached([rowAddress("r4")]);
  expect((await rows.cachedFeedView(deviceId)).items).toHaveLength(127);
  await cache.evictEntity(deviceId, "r5");
  expect((await rows.cachedFeedView(deviceId)).items).toHaveLength(126);
  await cache.wipeCache();
  expect((await reader.refresh()).view.items).toEqual([]);
});

it("retries a failed incremental read instead of losing the dirty address", async () => {
  await rows.cachedFeedView(deviceId);
  await cache.writeCached(rowAddress("r4"), row("r4", { muted: true }));
  vi.spyOn(cache, "readCachedMany").mockRejectedValueOnce(new Error("transient read failure"));
  await expect(rows.cachedFeedView(deviceId)).rejects.toThrow("transient read failure");
  expect((await rows.cachedFeedView(deviceId)).items.find((item) => item.entity_id === "r4").muted).toBe(true);
});

it("publishes a finite refresh while writes keep arriving under held reads", async () => {
  const reader = createFeedCacheReader(deviceId);
  await reader.refresh();
  await cache.writeCached(rowAddress("r4"), row("r4", { muted: true }));
  const readMany = cache.readCachedMany;
  const releases = [];
  const reads = vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => releases.push(resolve));
    return records;
  });
  let first;
  const pending = reader.refresh().then((value) => { first = value; });
  await vi.waitFor(() => expect(releases).toHaveLength(1));
  await cache.writeCached(rowAddress("r5"), row("r5", { muted: true }));
  // A consumer joining during this read must cover dirtiness at its own call.
  const trailing = rows.cachedFeedView(deviceId);
  releases[0]();
  try {
    await vi.waitFor(() => expect(first?.view.items.find((item) => item.entity_id === "r4").muted).toBe(true));
    await vi.waitFor(() => expect(releases).toHaveLength(2));
    await cache.writeCached(rowAddress("r6"), row("r6", { muted: true }));
    releases[1]();
    const trailingView = await trailing;
    expect(trailingView.items.find((item) => item.entity_id === "r5").muted).toBe(true);
    expect(reads).toHaveBeenCalledTimes(2);
    await pending;
  } finally {
    releases.forEach((release) => release());
  }
});

it("covers every address already dirty at a direct lookup, in bounded batches", async () => {
  await rows.cachedFeedView(deviceId);
  await Promise.all(Array.from({ length: 130 }, (_, index) =>
    cache.writeCached(rowAddress(`r${index}`), row(`r${index}`, { muted: true }))));
  const reads = vi.spyOn(cache, "readCachedMany");
  const view = await rows.cachedFeedView(deviceId);
  expect(view.items).toHaveLength(130);
  expect(view.items.every((item) => item.muted)).toBe(true);
  expect(reads.mock.calls.map(([addresses]) => addresses.length)).toEqual([64, 64, 2]);
});

it.each(["entity", "device"])("fences held responses after %s eviction", async (scope) => {
  await rows.cachedFeedView(deviceId);
  await cache.writeCached(rowAddress("r4"), row("r4", { muted: true }));
  const readMany = cache.readCachedMany;
  let release;
  vi.spyOn(cache, "readCachedMany").mockImplementationOnce(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => { release = resolve; });
    return records;
  });
  const pending = rows.cachedFeedView(deviceId);
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  if (scope === "entity") await cache.evictEntity(deviceId, "r4");
  else await cache.wipeCache();
  release();
  const view = await pending;
  expect(view.items.some((item) => item.entity_id === "r4")).toBe(false);
  expect(view.items).toHaveLength(scope === "entity" ? 127 : 0);
});
