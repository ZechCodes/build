// @vitest-environment jsdom
// Actual public feed/readiness behavior before any proposed lease export exists.
// These controls do not claim private frame/lease or immutable inspector coverage.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
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

describe("D5 current read-source readiness and equivalent rereads", () => {

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
