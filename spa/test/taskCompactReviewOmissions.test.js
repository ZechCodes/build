import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const scope = { deviceId: "compact-review-891", projectId: "proj-1" }, taskId = "task-1";
const review = { task_id: taskId, workspace_id: "workspace-1", version: 2, status: "open" };
// TaskView's native serialized shape: full dependency arrays, no summary-only
// count/truncation fields, and absent optional None fields (tasks.rs:753).
const fullTask = (extra = {}) => ({
  id: taskId, project_id: scope.projectId, number: 1, title: "Before", body: "Canonical body",
  state: "open", status: "ready", labels: [], priority: "none", assignee: null,
  links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null },
  trackers: [], identities: {}, attachments: [], watched: true, unread_count: 0,
  dependencies: { blocked: false, waiting_on: [], blocks_count: 0 },
  created_by: { kind: "user" }, created_at: "2026-10-29T11:00:00Z",
  updated_at: "2026-10-29T12:00:00Z", closed_at: null, ...extra,
});
let cache, content, tracker, summaries, actions, order;
const normalized = async () => (await cache.readCached(summaries.summaryAddress(scope, taskId))).value;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  content = await import("../src/core/taskContentAuthority.js");
  tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  actions = await import("../src/core/taskWaitingOnCache.js");
  order = await import("../src/core/taskReadOrder.js");
});

async function seedObservedReview() {
  const address = tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const contentRead = await content.captureTaskContentRead(address, { revalidate: "full" });
  const answer = { task: fullTask({ review_summary: review }), timeline: [{ id: "held-history" }] };
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, answer, { contentRead })).toBe(true);
  const row = await normalized();
  expect(row.task.review_summary).toEqual(review);
  expect(row.task).toHaveProperty("__task_watch_cache");
  expect(row.task.__task_content_cache.header.read_order).toBe(contentRead.readOrder);
  return answer;
}

it("retains the versioned review on a compact title-only patch after a genuine metadata-bearing full read", async () => {
  const before = await seedObservedReview();
  const read = await order.nextTaskRead();
  expect(await summaries.writeSummaryTask(scope, { id: taskId, title: "Compact title" }, read)).toBe(true);
  const row = await normalized();
  expect(row.task.title).toBe("Compact title");
  expect(row.task.review_summary).toEqual(review);
  expect(row.task.__review_summary_cache).toMatchObject({ floor: review, cleared: false });
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task.review_summary).toEqual(review);
  expect(detail.task.body).toBe(before.task.body);
  expect(detail.timeline).toEqual(before.timeline);
});

it("clears the versioned review on a genuine later full TaskView acknowledgment that omits it", async () => {
  const before = await seedObservedReview();
  const contentRead = await content.captureTaskContentRead(scope);
  const answer = { task: fullTask({ title: "Full title", updated_at: "2026-10-29T12:01:00Z" }) };
  expect(answer.task).not.toHaveProperty("review_summary");
  expect(await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, answer,
    { contentRead, issuedRead: contentRead.readOrder })).toBe(true);
  const row = await normalized();
  expect(row.task.title).toBe("Full title");
  expect(row.task).not.toHaveProperty("review_summary");
  expect(row.task.__review_summary_cache).toMatchObject({ floor: review, cleared: true });
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task).not.toHaveProperty("review_summary");
  expect(detail.task.body).toBe(before.task.body);
  expect(detail.timeline).toEqual(before.timeline);
});
