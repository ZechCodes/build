import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, tracker, summaries, actions, order;
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
const action = (task, options) => actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, { task }, options);
async function seed(task = base()) {
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, tracker.taskRecord({ ...task, body: "Held body", attachments: [] }, [{ id: "history" }]));
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([task], []));
  const generation = await order.nextTaskRead();
  const ticket = await summaries.beginSummaryQuery(scope, filters, generation);
  await summaries.landSummaryPage(ticket, { summary: true, tasks: [task], identities: {}, basis: "opaque-before", next_cursor: "kept-cursor" }, { read: generation });
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
});

it("commits assignment/dependency action summaries with legacy replicas and their same read order", async () => {
  await seed();
  const queryBefore = await value(summaries.summaryQueryAddress(scope, filters));
  const answer = full({ title: "Assigned", assignee: { kind: "agent", agent_id: "AgentA" },
    updated_at: "2026-10-29T12:01:00Z", dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 2,
      waiting_on: [{ task_id: "expanded" }], blocks: [{ task_id: "full dependent" }] },
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

it("treats full-task omissions as authoritative while compact patches retain omitted compact facts", async () => {
  await seed();
  await summaries.writeSummaryTask(scope, { id: taskId, title: "Compact edit", body: "not a summary" }, await order.nextTaskRead());
  let row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ watched: true, done_at: "yesterday", review_summary: { version: 2 }, deferred_assignment: { state: "waiting" } });
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
  await action(full({ title: "Older content", updated_at: "2026-10-29T12:00:00Z", read_through: "tc-21\0",
    watch_started_after: "", dismissed_through: null, unread_count: 9, unread_mentions: 3, unread_agent_comments: 4,
    review_summary: { task_id: taskId, version: 6, status: "closed", mode: "pull_request" } }));
  const row = (await value(summaries.summaryAddress(scope, taskId))).task;
  expect(row).toMatchObject({ title: "Newer content", read_through: "tc-21\0", watch_started_after: "te-30", dismissed_through: "te-40",
    unread_count: 0, unread_mentions: 0, unread_agent_comments: 0, review_summary: { version: 6, status: "closed" } });
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
  expect(fragment.addresses).toHaveLength(2);
  expect(await cache.mergeCachedRecordsTogether(fragment.addresses, fragment.reduce)).toBe(true);
  expect((await value(summaries.summaryAddress(scope, taskId))).task.title).toBe("Fragment");
  expect(() => summaries.summaryTaskMutation(scope, { id: taskId }, read, { fullTask: "yes" })).toThrow();
});

it("retains incomplete full-action counter families until a complete covering summary arrives", async () => {
  await seed();
  const answer = full({ read_through: "tc-20", updated_at: "2026-10-29T12:01:00Z" });
  for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) delete answer[field];
  await action(answer);
  let held = await value(summaries.summaryAddress(scope, taskId));
  expect(held).toMatchObject({ needs_unread_refresh: true, task: { read_through: "tc-20", unread_count: 3, unread_mentions: 1, unread_agent_comments: 2 } });
  async function refresh(task) {
    const read = await order.nextTaskRead(), ticket = await summaries.beginSummaryQuery(scope, filters, read);
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
});

it("compares raw full-action floors before merging held values and never certifies weaker counters", async () => {
  await seed(base({ read_through: "tc-20", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 }));
  const answer = full({ unread_count: 9, unread_mentions: 3, unread_agent_comments: 4, updated_at: "2026-10-29T12:01:00Z" });
  delete answer.read_through; delete answer.watch_started_after;
  await action(answer);
  expect(await value(summaries.summaryAddress(scope, taskId))).toMatchObject({ needs_unread_refresh: true,
    task: { read_through: "tc-20", unread_count: 0, unread_mentions: 0, unread_agent_comments: 0 } });
});

it("marks counter authority incomplete when a compact patch advances its floor without paired counters", async () => {
  await seed();
  await summaries.writeSummaryTask(scope, { id: taskId, read_through: "tc-20" }, await order.nextTaskRead());
  expect(await value(summaries.summaryAddress(scope, taskId))).toMatchObject({ needs_unread_refresh: true,
    task: { read_through: "tc-20", unread_count: 3, unread_mentions: 1, unread_agent_comments: 2 } });
  await action(full({ watched: false, updated_at: "2026-10-29T12:01:00Z" }));
  expect((await value(summaries.summaryAddress(scope, taskId))).needs_unread_refresh).toBe(false);
});

it("rejects an older issued action after a newer same-timestamp action committed", async () => {
  await seed(); const first = await order.nextTaskRead(), second = await order.nextTaskRead();
  await action(full({ watched: false, title: "Newer action" }), { issuedRead: second });
  const before = await snapshot();
  expect(await action(full({ watched: true, title: "Older action" }), { issuedRead: first })).toBe(false);
  expect(await snapshot()).toEqual(before);
});

it.each([true, false])("keeps newer content while accepting an older-body action's read floor and watched=%s", async (watched) => {
  await seed(base({ title: "Newer title", updated_at: "2026-10-29T13:00:00Z", watch_started_after: "" }));
  await action(full({ title: "Old title", body: "Old body", watched, read_through: "tc-30\0", watch_started_after: "",
    updated_at: "2026-10-29T12:00:00Z", unread_count: 0 }), { issuedRead: await order.nextTaskRead() });
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
