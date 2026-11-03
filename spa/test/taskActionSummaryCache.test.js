import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { rememberTaskUnreadDisplay } from "./taskUnreadDisplayFixture.js";

let cache, tracker, summaries, actions, order, content;
const scope = { deviceId: "action-summary-900", projectId: "proj-1" };
const taskId = "task-1", filters = { states: ["open"] };
const base = (extra = {}) => ({ id: taskId, project_id: scope.projectId, number: 1, title: "Before",
  state: "open", status: "ready", assignee: { kind: "user" }, watched: true,
  updated_at: "2026-10-29T12:00:00Z", read_through: "tc-10", watch_started_after: "te-15",
  dismissed_through: "te-05", unread_count: 3, unread_mentions: 1, unread_agent_comments: 2,
  session_started_ms: 1, last_activity_ms: 2, done_at: "yesterday", deferred_assignment: { state: "waiting" },
  review_summary: { task_id: taskId, version: 2, status: "open", mode: "pull_request" },
  dependencies: { blocked: true, waiting_on_count: 1, blocks_count: 0 },
  links: { workspace_ids: ["ws-1"], parent_task_id: null }, attachment_count: 0, ...extra });
const full = (extra = {}) => {
  const { attachment_count, ...task } = base();
  return { ...task, body: "Full canonical body", attachments: [], ...extra };
};
const detailAddress = () => tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
const invalidationAddress = () => ({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-summary-invalidation", sub: "" });
const value = async (address) => (await cache.readCached(address))?.value;
const unguardedAction = (task, options) => actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task }, options);
async function action(task, options = {}) {
  const contentRead = options.contentRead || await content.captureTaskContentRead(scope);
  return unguardedAction(task, { ...options, contentRead, issuedRead: contentRead.readOrder });
}
async function seed(task = base()) {
  const summaryRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const generation = summaryRead.readOrder;
  const ticket = await summaries.beginSummaryQuery(scope, filters, generation, { contentRead: summaryRead });
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [task], identities: {}, basis: "opaque-before", next_cursor: "kept-cursor" }, { read: generation });
  const heldTask = { ...task, body: "Held body", attachments: [] };
  delete heldTask.attachment_count;
  const catalogue = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const listRead = await content.captureTaskContentRead(catalogue, { revalidate: "full" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([
    content.observeTaskContent(heldTask, { contentRead: listRead, coverage: "full" }),
  ], [], listRead.readOrder));
  const detailRead = await content.captureTaskContentRead(detailAddress(), { revalidate: "full" });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(heldTask, [{ id: "history" }]), { contentRead: detailRead });
  return ticket;
}
async function snapshot() {
  return Promise.all([detailAddress(), tracker.tasksAddress(scope.deviceId, scope.projectId),
    order.readsAddress(detailAddress()), summaries.summaryAddress(scope, taskId), invalidationAddress(),
    summaries.summaryQueryAddress(scope, filters), actions.taskWaitingOnGenerationAddress(scope.deviceId, scope.projectId)].map(value));
}

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js"); actions = await import("../src/core/taskWaitingOnCache.js");
  order = await import("../src/core/taskReadOrder.js");
  content = await import("../src/core/taskContentAuthority.js");
});

it("commits assignment/dependency action summaries with legacy replicas and their same read order", async () => {
  await seed();
  const queryBefore = await value(summaries.summaryQueryAddress(scope, filters));
  const answer = full({ title: "Assigned", assignee: { kind: "agent", agent_id: "AgentA" },
    updated_at: "2026-10-29T12:01:00Z", dependencies: { blocked: false, blocks_count: 2,
      waiting_on: [], blocks: [{ task_id: "full dependent" }] },
    links: { workspace_ids: ["ws-2"], parent_task_id: "task-parent", commits: ["expanded history"] },
    attachments: [{ path: "large payload" }] });
  expect(await action(answer)).toBe(true);
  const row = await value(summaries.summaryAddress(scope, taskId));
  expect(row.task).toMatchObject({ title: "Assigned", assignee: answer.assignee, attachment_count: 1,
    dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 2 }, links: { workspace_ids: ["ws-2"], parent_task_id: "task-parent" } });
  expect(row.task).not.toHaveProperty("body"); expect(row.task).not.toHaveProperty("attachments");
  expect(row.task.dependencies).not.toHaveProperty("waiting_on"); expect(row.task.links).not.toHaveProperty("commits");
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task.body).toBe(answer.body); expect(detail.timeline).toEqual([{ id: "history" }]);
  const list = await tracker.readTasksRecord(scope.deviceId, scope.projectId);
  expect(list.tasks[0].assignee).toEqual(answer.assignee);
  expect(row.read_order).toBe(list.read_order);
  expect((await value(order.readsAddress(detailAddress()))).rows[taskId]).toBe(row.read_order);
  expect((await value(invalidationAddress())).read_order).toBe(row.read_order);
  expect(row).not.toHaveProperty("basis");
  expect(await value(summaries.summaryQueryAddress(scope, filters))).toEqual(queryBefore);
  expect(await summaries.readSummaryQuery(scope, filters)).toMatchObject({ stale: true, ids: [taskId], next_cursor: "kept-cursor" });
});

it("derives one waiting dependency from a complete native full-task relation array", async () => {
  await seed();
  const waiting = { task_id: "task-blocker", number: 2, title: "Blocker", state: "open",
    status: "ready", resolved: false, resolved_by: null };
  const answer = full({ updated_at: "2026-10-29T12:01:00Z",
    dependencies: { blocked: true, waiting_on: [waiting], blocks_count: 0 } });
  expect(await action(answer)).toBe(true);
  const row = await value(summaries.summaryAddress(scope, taskId));
  expect(row.task.dependencies).toEqual({ blocked: true, waiting_on_count: 1, blocks_count: 0 });
  expect((await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId)).task.dependencies.waiting_on).toEqual([waiting]);
});

it("treats full-task omissions as authoritative while compact patches retain omitted compact facts", async () => {
  await seed();
  await summaries.writeSummaryTask(scope, { id: taskId, title: "Compact edit", body: "not a summary" }, await order.nextTaskRead());
  let row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ watched: true, done_at: "yesterday", review_summary: { version: 2 }, deferred_assignment: { state: "waiting" } });
  // The local compact edit retired its header proof. Revalidate that header
  // before testing a separate confirmed action's full-task omissions.
  const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
  const ticket = await summaries.beginSummaryQuery(scope, filters, contentRead.readOrder, { contentRead });
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [base({ title: "Compact edit" })],
    basis: "compact-revalidated" }, { read: contentRead.readOrder });
  const answer = full({ title: "Full edit", updated_at: "2026-10-29T12:02:00Z" });
  for (const field of ["watched", "done_at", "review_summary", "deferred_assignment", "attachment_count"]) delete answer[field];
  answer.attachments = [{ path: "one" }, { path: "two" }];
  await action(answer);
  row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ title: "Full edit", watched: false, done_at: null, deferred_assignment: null, attachment_count: 2 });
  expect(row).not.toHaveProperty("review_summary");
  for (const field of ["watch_started_after", "unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) expect(row).not.toHaveProperty(field);
});

it("invalidates all earlier query generations without inventing task inclusion", async () => {
  const old = await seed();
  const emptyFilters = { statuses: ["done"] }, generation = await order.nextTaskRead();
  const empty = await summaries.beginSummaryQuery(scope, emptyFilters, generation);
  await summaries.landSummaryPage(empty, { summary: true, tasks: [], basis: "opaque-before" }, { read: generation });
  const newTask = full({ id: "task-new", number: 2, title: "Newly filed", status: "done" });
  await action(newTask);
  expect((await value(summaries.summaryAddress(scope, "task-new"))).task.title).toBe("Newly filed");
  expect(await summaries.readSummaryQuery(scope, emptyFilters)).toMatchObject({ ids: [], tasks: [], stale: true });
  expect((await summaries.readSummaryQuery(scope, filters)).ids).toEqual([taskId]);
  expect(await summaries.landSummaryPage({ ...old, cursor: "kept-cursor" }, { summary: true, tasks: [base()], basis: "opaque-before" }, { read: await order.nextTaskRead() })).toBe(false);
});

it("checks acceptance inside the joint transaction before any legacy or normalized publication", async () => {
  await seed(); const before = await snapshot(); let accepted = true;
  const original = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (String(key).includes("|tracker-summary|")) accepted = false;
    return original.call(this, key);
  });
  expect(await action(full({ title: "Rejected" }), { accept: () => accepted })).toBe(false);
  spy.mockRestore(); expect(await snapshot()).toEqual(before);
});

it("rolls back legacy details/lists/notes when normalized summary publication hits quota", async () => {
  await seed(); const before = await snapshot();
  const original = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (String(key).includes("|tracker-summary|") && record?.value?.task?.title === "Quota refused") {
      throw new DOMException("Full quota", "QuotaExceededError");
    }
    return original.call(this, record, key);
  });
  expect(await action(full({ title: "Quota refused", updated_at: "2026-10-29T12:01:00Z" }))).toBe(false);
  spy.mockRestore(); expect(await snapshot()).toEqual(before);
});

it("preserves newer ordinary task fields and exact marker/counter authority while a higher PR version advances", async () => {
  await seed(base({ title: "Newer content", updated_at: "2026-10-29T13:00:00Z", read_through: "tc-20\0",
    watch_started_after: "te-30", dismissed_through: "te-40", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0,
    review_summary: { task_id: taskId, version: 5, status: "open", mode: "pull_request" } }));
  // This stale legacy observation has no confirmed watch-lineage dispatch
  // authority; PR versions and monotonic floors still advance independently.
  await unguardedAction(full({ title: "Older content", updated_at: "2026-10-29T12:00:00Z", read_through: "tc-21\0",
    watch_started_after: "", dismissed_through: null, unread_count: 9, unread_mentions: 3, unread_agent_comments: 4,
    review_summary: { task_id: taskId, version: 6, status: "closed", mode: "pull_request" } }));
  const row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ title: "Newer content", read_through: "tc-21\0", watch_started_after: "te-30", dismissed_through: "te-40",
    unread_count: 0, review_summary: { version: 6, status: "closed" } });
  expect(row).not.toHaveProperty("unread_mentions"); expect(row).not.toHaveProperty("unread_agent_comments");
  expect((await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId)).task.title).toBe("Newer content");
  expect((await summaries.readSummaryQuery(scope, filters)).stale).toBe(true);
});

it("ends unwatch counter lineage and accepts a later present-empty watch floor", async () => {
  await seed();
  await action(full({ watched: false, updated_at: "2026-10-29T12:01:00Z" }));
  let row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row.watched).toBe(false); expect(row).not.toHaveProperty("watch_started_after"); expect(row).not.toHaveProperty("unread_count");
  await action(full({ watched: true, watch_started_after: "", unread_count: 2, updated_at: "2026-10-29T12:02:00Z" }));
  row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ watched: true, watch_started_after: "", unread_count: 2 });
});

it("rejects a foreign project action without changing either replica family", async () => {
  await seed(); const before = await snapshot();
  await expect(action(full({ project_id: "other-project" }))).rejects.toThrow();
  expect(await snapshot()).toEqual(before);
});

it("exposes a guarded bounded mutation fragment that can share a caller transaction", async () => {
  await seed();
  const read = await order.nextTaskRead(), fragment = summaries.summaryTaskMutation(scope, { id: taskId, title: "Fragment" }, read);
  expect(fragment.addresses).toEqual([summaries.summaryAddress(scope, taskId), invalidationAddress(),
    content.TASK_CONTENT_GENERATION_ADDRESS, content.taskContentScopeAddress(scope),
    content.taskWatchControlAddress(scope), order.taskWatchIntentAddress(scope, taskId)]);
  expect(await cache.mergeCachedRecordsTogether(fragment.addresses, fragment.reduce)).toBe(true);
  expect((await value(summaries.summaryAddress(scope, taskId))).task.title).toBe("Fragment");
  const before = await snapshot();
  const refused = summaries.summaryTaskMutation(scope, { id: taskId, title: "Refused fragment" }, read, { accept: () => false });
  expect(await cache.mergeCachedRecordsTogether(refused.addresses, refused.reduce)).toBe(false);
  expect(await snapshot()).toEqual(before);
  expect(() => summaries.summaryTaskMutation(scope, { id: taskId }, read, { fullTask: "yes" })).toThrow();
});

it("retains incomplete full-action counter families until a complete covering summary arrives", async () => {
  await seed();
  const display = await rememberTaskUnreadDisplay(scope, (await value(summaries.summaryAddress(scope, taskId))).task);
  const answer = full({ read_through: "tc-20", updated_at: "2026-10-29T12:01:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete answer[field];
  await action(answer);
  let held = await value(summaries.summaryAddress(scope, taskId));
  expect(held).toMatchObject({ needs_unread_refresh: true, task: { read_through: "tc-20" } });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) expect(held.task).not.toHaveProperty(field);
  expect(display(held.task)).toBe(3);
  async function refresh(task) {
    const contentRead = await content.captureTaskContentRead(scope, { revalidate: "summary" });
    const read = contentRead.readOrder, ticket = await summaries.beginSummaryQuery(scope, filters, read, { contentRead });
    await summaries.landSummaryPage(ticket, { summary: true, tasks: [task], basis: "opaque-after" }, { read });
  }
  const incomplete = { ...base(), updated_at: answer.updated_at, read_through: "tc-20", unread_count: 0 };
  delete incomplete.unread_mentions; delete incomplete.unread_agent_comments;
  await refresh(incomplete);
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(true);
  await refresh({ ...incomplete, unread_mentions: 0, unread_agent_comments: 0 });
  held = await value(summaries.summaryAddress(scope, taskId));
  expect(held.needs_unread_refresh).toBe(false);
  expect(held.task).toMatchObject({ unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 });
  expect(display(held.task)).toBe(0);
});

it("compares raw full-action floors before merging held values and never certifies weaker counters", async () => {
  await seed(base({ read_through: "tc-20", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 }));
  const answer = full({ unread_count: 9, unread_mentions: 3, unread_agent_comments: 4, updated_at: "2026-10-29T12:01:00Z" });
  delete answer.read_through; delete answer.watch_started_after;
  // This legacy absent-floor observation has no confirmed watch-lineage
  // dispatch authority; it must retain stronger held floors and counters.
  await unguardedAction(answer);
  expect(await value(summaries.summaryAddress(scope, taskId))).toMatchObject({ needs_unread_refresh: false,
    task: { read_through: "tc-20", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 } });
});

it("marks counter authority incomplete when a compact patch advances its floor without paired counters", async () => {
  await seed();
  const display = await rememberTaskUnreadDisplay(scope, (await value(summaries.summaryAddress(scope, taskId))).task);
  await summaries.writeSummaryTask(scope, { id: taskId, read_through: "tc-20" }, await order.nextTaskRead());
  expect(await value(summaries.summaryAddress(scope, taskId))).toMatchObject({ needs_unread_refresh: true,
    task: { read_through: "tc-20" } });
  const unknown = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(unknown).not.toHaveProperty("unread_count");
  expect(display(unknown)).toBe(3);
  await action(full({ watched: false, updated_at: "2026-10-29T12:01:00Z" }));
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(false);
});

it("rejects an older issued action after a newer same-timestamp action committed", async () => {
  await seed();
  const first = await content.captureTaskContentRead(scope), second = await content.captureTaskContentRead(scope);
  await action(full({ watched: false, title: "Newer action" }), { contentRead: second });
  const before = await snapshot();
  expect(await action(full({ watched: true, title: "Older action" }), { contentRead: first })).toBe(false);
  expect(await snapshot()).toEqual(before);
});

it.each([true, false])("keeps newer content while accepting an older-body action's read floor and watched=%s", async (watched) => {
  await seed(base({ title: "Newer title", updated_at: "2026-10-29T13:00:00Z", watch_started_after: "" }));
  await action(full({ title: "Old title", body: "Old body", watched, read_through: "tc-30\0", watch_started_after: "",
    updated_at: "2026-10-29T12:00:00Z", unread_count: 0 }));
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task).toMatchObject({ title: "Newer title", body: "Held body", read_through: "tc-30\0", watched });
  expect(detail.timeline).toEqual([{ id: "history" }]);
  const row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ title: "Newer title", read_through: "tc-30\0", watched });
  const listed = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
  expect(listed).toMatchObject({ title: "Newer title", read_through: "tc-30\0", watched });
  if (!watched) {
    for (const task of [row, detail.task, listed]) {
      expect(task).not.toHaveProperty("watch_started_after"); expect(task).not.toHaveProperty("unread_count");
    }
  }
});

it("does not let a complete action patch certify previously uncertain counter authority", async () => {
  await seed();
  const incomplete = full({ read_through: "tc-20" });
  delete incomplete.unread_mentions; delete incomplete.unread_agent_comments;
  await action(incomplete);
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(true);
  await summaries.writeSummaryTask(scope, { id: taskId, read_through: "tc-20", unread_count: 0,
    unread_mentions: 0, unread_agent_comments: 0 }, await order.nextTaskRead());
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(true);
});

it("does not grant action watch authority to a stale watched summary page after unwatch", async () => {
  await seed(base({ updated_at: "2026-10-29T13:00:00Z" }));
  const generation = await order.nextTaskRead(), query = { statuses: ["ready"] };
  const page = await summaries.beginSummaryQuery(scope, query, generation);
  await action(full({ watched: false, updated_at: "2026-10-29T12:00:00Z" }));
  expect((await value(summaries.summaryAddress(scope, taskId))).task.watched).toBe(false);
  expect(await summaries.landSummaryPage(page, { summary: true, basis: "opaque-before", tasks: [base()] }, { read: generation })).toBe(false);
  expect((await value(summaries.summaryAddress(scope, taskId))).task.watched).toBe(false);
});
