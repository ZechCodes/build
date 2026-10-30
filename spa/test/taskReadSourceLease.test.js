// @vitest-environment jsdom
// Actual public feed/readiness behavior before any proposed lease export exists.
// These controls do not claim private frame/lease or immutable inspector coverage.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { task as wireTask } from "./trackerWireFixture.js";

const scope = { deviceId: "read-source-device", projectId: "proj-1" };
const taskId = "read-source-task";
const WAIT = { timeout: 3000, interval: 10 };
const listed = (updated_at = "2026-10-29T12:00:00Z") => ({ id: taskId, updated_at });
const detail = () => ({ task: wireTask({ ...listed(), project_id: scope.projectId, watched: true,
  read_through: "tc-10", watch_started_after: "te-15", dismissed_through: "te-05",
  unread_count: 3, attachments: [] }), timeline: [] });

let cache, tracker, createFeed, feed;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  createFeed = (await import("../src/core/trackerTaskDetailsFeed.js")).createTrackerTaskDetailsFeed;
  feed = null;
});

afterEach(() => {
  feed?.dispose();
  vi.restoreAllMocks();
});

async function traceActualDetailReads(address) {
  const key = (await import("../src/core/idbRecords.js")).recordKey(address);
  const get = IDBObjectStore.prototype.get;
  const reads = [];
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (requested) {
    const request = get.call(this, requested);
    if (this.name === "records" && requested === key) {
      const transaction = this.transaction;
      request.addEventListener("success", () => reads.push({
        db: transaction.db, mode: transaction.mode, record: structuredClone(request.result),
      }));
    }
    return request;
  });
  return reads;
}

async function scanTwice(onChange) {
  feed = createFeed({ ...scope, onChange });
  expect(feed.read().has(taskId)).toBe(false);
  expect(onChange).not.toHaveBeenCalled();
  await feed.updateTasks([listed()]);
  // A new listed version legitimately asks the existing cache-only owner to
  // rescan the SAME detail group, without a wire call or a fabricated hint.
  await feed.updateTasks([listed("2026-10-29T12:01:00Z")]);
}

describe("D5 current read-source readiness and equivalent rereads", () => {
  it("cues one draw when a real scan first learns absent detail and stays quiet on an equivalent reread", async () => {
    const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    expect(await cache.readCanonicalCached(address)).toBeUndefined();
    const reads = await traceActualDetailReads(address);
    const onChange = vi.fn();
    await scanTwice(onChange);
    expect(reads).toHaveLength(2);
    expect(reads.map((read) => read.mode)).toEqual(["readonly", "readonly"]);
    expect(reads[0].db).toBe(reads[1].db);
    expect(reads.map((read) => read.record)).toEqual([undefined, undefined]);
    expect(feed.read().has(taskId)).toBe(false);
    // Known absence is read readiness, not a detail object or unread zero.
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("keeps explicit null separate from absence without cueing again for the same actual null record", async () => {
    const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    await cache.writeCached(address, null);
    const stored = await cache.readCanonicalCached(address);
    expect(stored).toHaveProperty("value", null);
    const reads = await traceActualDetailReads(address);
    const onChange = vi.fn();
    await scanTwice(onChange);
    expect(reads).toHaveLength(2);
    expect(reads.map((read) => Object.hasOwn(read.record, "value"))).toEqual([true, true]);
    expect(reads.map((read) => read.record.value)).toEqual([null, null]);
    expect(reads.map((read) => cache.recordWriteOf(read.record))).toEqual([
      cache.recordWriteOf(stored), cache.recordWriteOf(stored),
    ]);
    expect(feed.read().has(taskId)).toBe(false);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("cues a fresh cached selection when the real source write changes but ordinary task data stays equal", async () => {
    const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
    const value = detail();
    // An ordinary migrated record, no private proof/display metadata seeded.
    await cache.writeCached(address, value);
    const before = await cache.readCanonicalCached(address);
    const onChange = vi.fn();
    feed = createFeed({ ...scope, onChange });
    await feed.updateTasks([listed()]);
    expect(feed.read().get(taskId)).toEqual(value);
    expect(onChange).toHaveBeenCalledTimes(1);
    await cache.writeCached(address, structuredClone(value));
    const after = await cache.readCanonicalCached(address);
    expect(cache.recordWriteOf(after)).not.toBe(cache.recordWriteOf(before));
    expect(after.value).toEqual(before.value);
    await vi.waitFor(() => expect(onChange).toHaveBeenCalledTimes(2), WAIT);
    expect(feed.read().get(taskId)).toEqual(value);
    expect(feed.read().get(taskId).task.unread_count).toBe(3);
  });
});
