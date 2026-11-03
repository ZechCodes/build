import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const scope = { deviceId: "legacy-watch-authority", projectId: "proj-1" };
const taskId = "legacy-watch-task";
const full = (extra = {}) => task({ id: taskId, project_id: scope.projectId, body: "Full body",
  watched: true, watch_started_after: "te-05", read_through: "tc-10", unread_count: 3,
  unread_mentions: 0, unread_agent_comments: 0, session_started_ms: null, last_activity_ms: null, ...extra });
let cache, content, tracker, summaries, watch;

async function open() {
  cache = await import("../src/core/localCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  watch = await import("../src/core/taskWatchAuthority.js");
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  await open();
});

async function publish(row) {
  const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(row, []), { contentRead })).toBe(true);
  return (await cache.readCanonicalCached(summaries.summaryAddress(scope, taskId))).value;
}

it("admits ordinary covering watch and counter facts on first open and after reload", async () => {
  expect(await publish(full())).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false,
    task: { watched: true, unread_count: 3 } });
  vi.resetModules();
  await open();
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, eligible: true });
  expect(await publish(full({ read_through: "tc-20", unread_count: 0 }))).toMatchObject({
    watch_uncertain: false, needs_unread_refresh: false, task: { unread_count: 0, read_through: "tc-20" } });
});

it("protects an in-flight watch intent and admits the next covering read after settlement", async () => {
  await publish(full());
  const owner = await watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(owner).not.toBeNull();
  expect(await publish(full({ unread_count: 9 }))).toMatchObject({ watch_uncertain: true });
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, eligible: false });
  await watch.settleTaskWatchMutation(owner, { rollback: true });
  expect(await publish(full({ read_through: "tc-20", unread_count: 0 }))).toMatchObject({
    watch_uncertain: false, needs_unread_refresh: false, task: { watched: true, unread_count: 0 } });
});
