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

it("resolves a cold route while unrelated feed roots remain held", async () => {
  await cache.writeCached(rowAddress("r4"), row("r4", { deviceId, projectKey: `${deviceId}/p1` }));
  const read = cache.readCached;
  let release;
  vi.spyOn(cache, "readCached").mockImplementation(async (address) => {
    const record = await read(address);
    if (address.kind === "feed") await new Promise((resolve) => { release = resolve; });
    return record;
  });
  let route, feed;
  const feeding = createFeedCacheReader(deviceId).refresh().then((value) => { feed = value; });
  const routing = rows.cachedRouteEntry(deviceId, { name: "branch", deviceId, projectId: "p1", branch: "r4" })
    .then((value) => { route = value; });
  try {
    await vi.waitFor(() => expect(release).toBeTypeOf("function"), { timeout: 10000 });
    await vi.waitFor(() => expect(route?.entityId).toBe("r4"), { timeout: 10000 });
    expect(feed).toBeUndefined();
  } finally {
    release?.();
    await Promise.all([feeding, routing]);
  }
  expect(feed.view.items).toHaveLength(128);
});

it("does not combine an old owner snapshot with auxiliary roots read across a reset", async () => {
  const read = cache.readCached;
  let release;
  vi.spyOn(cache, "readCached").mockImplementation(async (address) => {
    const record = await read(address);
    if (address.kind === "feed" && !release) await new Promise((resolve) => { release = resolve; });
    return record;
  });
  const feeding = createFeedCacheReader(deviceId).refresh();
  await rows.cachedFeedView(deviceId, { projectRosters: false });
  try {
    await vi.waitFor(() => expect(release).toBeTypeOf("function"), { timeout: 10000 });
    await cache.wipeCache();
    await cache.writeCached(rowAddress("replacement"), row("replacement"));
  } finally {
    release?.();
  }
  expect((await feeding).view.items.map((item) => item.entity_id)).toEqual(["replacement"]);
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

it("bounds row reads while serial entity departures retire retained board rows", async () => {
  await Promise.all(Array.from({ length: 76 }, (_, index) => cache.writeCached(rowAddress(`r${index + 128}`), row(`r${index + 128}`))));
  await rows.cachedFeedView(deviceId);
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  for (let index = 6; index < 204; index += 1) {
    await cache.evictEntity(deviceId, `r${index}`);
    await cache.writeCached(rowAddress("r0"), row("r0", { revision: index }));
    const [feed, route] = await Promise.all([
      rows.cachedFeedView(deviceId), rows.cachedFeedView(deviceId, { projectRosters: false }),
    ]);
    expect(feed.items).toHaveLength(203 - (index - 6));
    expect(route.items).toEqual(feed.items);
  }
  // Only changed live rows and departed addresses need reads, never a whole
  // remaining-row sweep for every departure (20,691 gets before the fix).
  expect(rowReads(gets)).toBeLessThanOrEqual(400);
}, 60000);

it("publishes a finite held pass while a different entity leaves", async () => {
  await rows.cachedFeedView(deviceId);
  await cache.writeCached(rowAddress("r4"), row("r4", { muted: true }));
  const readMany = cache.readCachedMany;
  const releases = [];
  vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => releases.push(resolve));
    return records;
  });
  let view;
  const pending = rows.cachedFeedView(deviceId).then((next) => { view = next; });
  await vi.waitFor(() => expect(releases).toHaveLength(1));
  await cache.evictEntity(deviceId, "r5");
  await cache.writeCached(rowAddress("r6"), row("r6", { muted: true }));
  releases[0]();
  try {
    await vi.waitFor(() => expect(view).toBeDefined());
    expect(view.items.find((item) => item.entity_id === "r4").muted).toBe(true);
    expect(view.items.some((item) => item.entity_id === "r5")).toBe(false);
  } finally {
    releases.forEach((release) => release());
    await pending;
  }
});

it("keeps recreated entity writes dirty after rejecting an older held response", async () => {
  await rows.cachedFeedView(deviceId);
  await cache.writeCached(rowAddress("r4"), row("r4", { label: "old held row" }));
  const readMany = cache.readCachedMany;
  let release;
  vi.spyOn(cache, "readCachedMany").mockImplementationOnce(async (addresses) => {
    const records = await readMany(addresses);
    await new Promise((resolve) => { release = resolve; });
    return records;
  });
  const pending = rows.cachedFeedView(deviceId);
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  await cache.evictEntity(deviceId, "r4");
  await cache.writeCached(rowAddress("r4"), row("r4", { label: "recreated row" }));
  release();
  const first = await pending;
  expect(first.items.find((item) => item.entity_id === "r4")?.label).not.toBe("old held row");
  const current = await rows.cachedFeedView(deviceId);
  expect(current.items.find((item) => item.entity_id === "r4").label).toBe("recreated row");
});

it("does not reload rows or auxiliary roots for an unrelated entity eviction", async () => {
  await cache.writeCached({ deviceId, entityId: "thread-only", kind: "thread" }, { messages: [] });
  await rows.cachedFeedView(deviceId);
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  await cache.evictEntity(deviceId, "thread-only");
  const view = await rows.cachedFeedView(deviceId);
  expect(view.items).toHaveLength(128);
  expect(gets).not.toHaveBeenCalled();
});

it("does not combine departed owner rows with auxiliary roots held across recreation", async () => {
  await cache.writeCached(rowAddress("r4"), row("r4", { label: "departed owner row" }));
  await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [] });
  const read = cache.readCached;
  let release;
  vi.spyOn(cache, "readCached").mockImplementation(async (address) => {
    const record = await read(address);
    if (address.kind === "feed" && !release) await new Promise((resolve) => { release = resolve; });
    return record;
  });
  const feeding = createFeedCacheReader(deviceId).refresh();
  await rows.cachedFeedView(deviceId, { projectRosters: false });
  await vi.waitFor(() => expect(release).toBeTypeOf("function"));
  // Replace the current projection entry while the older owner snapshot is
  // still held by the auxiliary join, then retire and recreate that owner.
  await cache.writeCached(rowAddress("r4"), row("r4", { label: "intermediate replacement" }));
  await rows.cachedFeedView(deviceId, { projectRosters: false });
  await cache.evictEntity(deviceId, "r4");
  await cache.writeCached(rowAddress("r4"), row("r4", { label: "recreated after owners resolved" }));
  const route = await rows.cachedFeedView(deviceId, { projectRosters: false });
  expect(route.items.find((item) => item.entity_id === "r4").label).toBe("recreated after owners resolved");
  release();
  const combined = await feeding;
  expect(combined.view.items.find((item) => item.entity_id === "r4")?.label).not.toBe("departed owner row");
  const current = await createFeedCacheReader(deviceId).refresh();
  expect(current.view.items.find((item) => item.entity_id === "r4").label).toBe("recreated after owners resolved");
});

it("invalidates owner and auxiliary roots after an empty-entity prefix eviction", async () => {
  await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [{ id: "p1" }]);
  await cache.writeCached({ deviceId, entityId: "", kind: "agent-lineage-support" }, { namesMakers: true });
  const reader = createFeedCacheReader(deviceId);
  expect((await reader.refresh()).namesMakers).toBe(true);
  await cache.evictEntity(deviceId, "");
  const current = await reader.refresh();
  expect(current.view.projects).toEqual([]);
  expect(current.namesMakers).toBe(false);
  expect(current.view.items).toHaveLength(128);
});
