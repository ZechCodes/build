// #129: the Issues tab paints a list and when the cache took it from ONE read
// of the record, so the record cannot go between two reads and leave a list
// painted under a stamp of 0.
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const DEVICE = "dev-129";
const PROJECT = "proj-1";
const PARAMS = { project_id: PROJECT, state: "open" };
const listed = { id: "issue-1", number: 1, title: "One" };

let cache, trackerCache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
});

it("reads the whole list and its stamp together", async () => {
  await trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord([listed], []));
  const held = await cache.readCached(trackerCache.issuesAddress(DEVICE, PROJECT));
  expect(await trackerCache.readIssuesCached(DEVICE, PROJECT)).toEqual({ at: held.at, value: held.value });
});

it("reads a narrowed list and its stamp together", async () => {
  await trackerCache.writeIssuesQueryRecord(DEVICE, PROJECT, PARAMS, trackerCache.issuesRecord([listed], []));
  const held = await cache.readCached(trackerCache.issuesQueryAddress(DEVICE, PROJECT, PARAMS));
  expect(await trackerCache.readIssuesQueryCached(DEVICE, PROJECT, PARAMS)).toEqual({ at: held.at, value: held.value });
});

it("answers null for a list the cache has never held", async () => {
  expect(await trackerCache.readIssuesCached(DEVICE, PROJECT)).toBeNull();
  expect(await trackerCache.readIssuesQueryCached(DEVICE, PROJECT, PARAMS)).toBeNull();
});
