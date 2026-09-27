// #129: the Tasks tab paints a list and when the cache took it from ONE read
// of the record, so the record cannot go between two reads and leave a list
// painted under a stamp of 0.
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const DEVICE = "dev-129";
const PROJECT = "proj-1";
const PARAMS = { project_id: PROJECT, state: "open" };
const listed = { id: "task-1", number: 1, title: "One" };

let cache, trackerCache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
});

it("reads the whole list and its stamp together", async () => {
  await trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord([listed], []));
  const held = await cache.readCached(trackerCache.tasksAddress(DEVICE, PROJECT));
  expect(await trackerCache.readTasksCached(DEVICE, PROJECT)).toEqual({ at: held.at, value: held.value });
});

it("reads a narrowed list and its stamp together", async () => {
  await trackerCache.writeTasksQueryRecord(DEVICE, PROJECT, PARAMS, trackerCache.tasksRecord([listed], []));
  const held = await cache.readCached(trackerCache.tasksQueryAddress(DEVICE, PROJECT, PARAMS));
  expect(await trackerCache.readTasksQueryCached(DEVICE, PROJECT, PARAMS)).toEqual({ at: held.at, value: held.value });
});

it("answers null for a list the cache has never held", async () => {
  expect(await trackerCache.readTasksCached(DEVICE, PROJECT)).toBeNull();
  expect(await trackerCache.readTasksQueryCached(DEVICE, PROJECT, PARAMS)).toBeNull();
});
