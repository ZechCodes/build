import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, sync, summaries, mutations;
const deviceId = "sync-device", projectId = "proj-1";
const scope = { kind: "project_tasks", project_id: projectId, filters: { states: ["open"] }, order: "number" };
const place = { deviceId, projectId };
const task = (id, extra = {}) => ({ id, project_id: projectId, number: Number(id.slice(-1)), title: id,
  state: "open", watched: false, updated_at: "2026-11-03T12:00:00Z", ...extra });
const record = (id, revision = "1", extra = {}) => ({ kind: "task", key: id, revision, member: true,
  row: task(id), ...extra });
const response = (cursor = "opaque-cursor", extra = {}) => ({ incarnation: "incarnation", cursor,
  watermark: "1", head: "2", bridge_epoch: "epoch", ...extra });
const create = (definition = scope) => sync.createScopedTaskSyncCache({ deviceId, projectId, scope: definition, bridgeIdentity: deviceId });
const publish = async (writer, records, options = {}) => writer.publish({ response: response(), records,
  capture: await writer.capture(), ...options });
const raw = async (address) => (await cache.readCached(address))?.value;
const ids = async (filters = scope.filters) => (await raw(summaries.summaryQueryAddress(place, filters)))?.ids;
const legacy = (params) => ({ deviceId, entityId: projectId, kind: "tracker-tasks-query", sub: JSON.stringify(params) });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/scopedTaskSyncCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  mutations = await import("../src/core/taskMutationFence.js");
});

it("publishes a complete snapshot membership and its opaque cursor together", async () => {
  const writer = create();
  expect(await publish(writer, [record("task-1"), record("task-2")], { snapshot: true })).toBe(true);
  expect(await ids()).toEqual(["task-2", "task-1"]);
  expect((await summaries.readSummaryQuery(place, scope.filters)).stale).toBe(false);
  expect(await writer.readCursor()).toMatchObject({ incarnation: "incarnation", cursor: "opaque-cursor", head: "1", bridgeIdentity: deviceId });
  await publish(writer, [record("task-2", "2")], { snapshot: true, expectedCursor: "opaque-cursor", response: response("replacement") });
  expect(await ids()).toEqual(["task-2"]);
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.id).toBe("task-1");
});

it("rolls back rows and membership when cursor persistence fails; reread applies once", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  const captured = await writer.capture();
  const put = IDBObjectStore.prototype.put;
  const refusal = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(value, key) {
    if (String(key).includes("sync-cursor")) throw new DOMException("full", "QuotaExceededError");
    return put.call(this, value, key);
  });
  try {
    expect(await writer.publish({ response: response("second"), records: [record("task-2", "2")],
      expectedCursor: "opaque-cursor", capture: captured })).toBe(false);
  } finally { refusal.mockRestore(); }
  expect((await writer.readCursor()).cursor).toBe("opaque-cursor");
  expect(await raw(summaries.summaryAddress(place, "task-2"))).toBeUndefined();
  expect(await ids()).toEqual(["task-1"]);
  expect(await publish(writer, [record("task-2", "2")], { expectedCursor: "opaque-cursor", response: response("second") })).toBe(true);
  expect(await ids()).toEqual(["task-2", "task-1"]);
});

it("removes filter exits from every cached spelling of the same query, preserving other scopes", async () => {
  const writer = create();
  const scalar = legacy({ project_id: projectId, state: "open" });
  const plural = legacy({ project_id: projectId, states: ["open"], limit: 30 });
  const closed = legacy({ project_id: projectId, state: "closed" });
  for (const address of [scalar, plural, closed]) await cache.writeCached(address, { tasks: [task("task-1")] });
  await publish(writer, [record("task-1")], { snapshot: true });
  await publish(writer, [record("task-1", "2", { member: false, row: task("task-1", { state: "closed" }) })],
    { expectedCursor: "opaque-cursor", response: response("exit") });
  expect(await ids()).toEqual([]);
  expect((await raw(scalar)).tasks).toEqual([]);
  expect((await raw(plural)).tasks).toEqual([]);
  expect((await raw(closed)).tasks).toHaveLength(1);
});

it("deletes entities and all cached memberships, with a revision tombstone against resurrection", async () => {
  const writer = create();
  const other = summaries.summaryQueryAddress(place, { state: "closed" });
  await cache.writeCached(other, { ids: ["task-1"] });
  await publish(writer, [record("task-1", "7")], { snapshot: true });
  await publish(writer, [record("task-1", "8", { operation: "delete", member: false, row: null })],
    { expectedCursor: "opaque-cursor", response: response("deleted") });
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
  expect((await raw(other)).ids).toEqual([]);
  await publish(writer, [record("task-1", "7")], { expectedCursor: "deleted", response: response("redelivered") });
  expect(await ids()).toEqual([]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
  await publish(writer, [record("task-1", "8")], { expectedCursor: "redelivered", response: response("equal-revision") });
  expect(await ids()).toEqual([]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
});

it("persists invalidation before advancing the cursor", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  await publish(writer, [record("task-1", "2", { operation: "invalidate", row: null })],
    { expectedCursor: "opaque-cursor", response: response("invalidated") });
  expect((await raw(summaries.summaryAddress(place, "task-1"))).stale).toBe(true);
  expect((await raw(summaries.summaryQueryAddress(place, scope.filters))).stale).toBe(true);
  expect((await summaries.readSummaryQuery(place, scope.filters)).stale).toBe(true);
  expect((await writer.readCursor()).cursor).toBe("invalidated");
});

it("compares row revisions rather than delivery sequence and refuses an overtaken cursor", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "100", { seq: "1", row: task("task-1", { title: "new" }) })], { snapshot: true });
  await publish(writer, [record("task-1", "99", { seq: "999", row: task("task-1", { title: "old" }) })],
    { expectedCursor: "opaque-cursor", response: response("next") });
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.title).toBe("new");
  expect(await publish(writer, [record("task-1", "101")], { expectedCursor: "opaque-cursor" })).toBe(false);
  expect((await writer.readCursor()).cursor).toBe("next");
});

it("rejects an old cache-generation response even after an empty cache is recreated", async () => {
  const writer = create(), captured = await writer.capture();
  await cache.wipeCache();
  expect(await writer.publish({ response: response(), records: [record("task-1")], capture: captured, snapshot: true })).toBe(false);
  expect(await writer.readCursor()).toBeUndefined();
});

it("leaves optimistic mutations and their cursor untouched until a fresh post-mutation read", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  const captured = await writer.capture();
  let settle;
  const pending = mutations.wrapTaskMutationCall(deviceId, "tasks.update", () => new Promise((resolve) => { settle = resolve; }),
    { projectId, taskId: "task-1" });
  await cache.writeCached(summaries.summaryAddress(place, "task-1"), { task: task("task-1", { title: "optimistic" }) });
  expect(await writer.publish({ response: response("would-skip"), records: [record("task-1", "2")],
    capture: captured, expectedCursor: "opaque-cursor" })).toBe(false);
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.title).toBe("optimistic");
  settle(); await pending;
  expect(await writer.publish({ response: response("would-skip"), records: [record("task-1", "2")],
    capture: captured, expectedCursor: "opaque-cursor" })).toBe(false);
  expect((await writer.readCursor()).cursor).toBe("opaque-cursor");
});

it("publishes task heads as summary coverage without erasing an already cached body", async () => {
  const writer = create({ kind: "task", task_id: "task-1" });
  const detail = { deviceId, entityId: projectId, kind: "tracker-task", sub: "task-1" };
  await cache.writeCached(detail, { task: task("task-1", { body: "saved body" }), timeline: ["old"] });
  await publish(writer, [record("task-1", "4", { kind: "task_head", row: { task: task("task-1", { title: "head" }), timeline: ["new"], has_more: true } })], { snapshot: true });
  expect(await raw(detail)).toMatchObject({ task: { title: "head", body: "saved body" }, timeline: ["new"], has_more: true });
});

it("removes just the affected scope and retires a removed task through content authority", async () => {
  const writer = create(), other = create({ kind: "task", task_id: "task-1" });
  await publish(writer, [record("task-1")], { snapshot: true });
  await publish(other, [record("task-1", "1", { kind: "task_head", row: { task: task("task-1"), timeline: [] } })], { snapshot: true });
  const membership = await cache.readCached(summaries.summaryQueryAddress(place, scope.filters));
  expect(await other.publish({ response: response(), records: [], expectedCursor: "opaque-cursor",
    capture: await other.capture(), removed: true })).toBe(true);
  expect(await other.readCursor()).toBeUndefined();
  expect((await writer.readCursor()).cursor).toBe("opaque-cursor");
  expect((await cache.readCached(summaries.summaryQueryAddress(place, scope.filters))).write).toBe(membership.write);
  expect(await raw({ deviceId, entityId: projectId, kind: "tracker-task", sub: "task-1" })).toMatchObject({ task: null });
});

it("separates the seen head from the fully applied head across a continuation", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  await publish(writer, [record("task-1", "2")], { expectedCursor: "opaque-cursor",
    response: response("middle", { head: "5", continuation: "fixed-head" }) });
  expect(await writer.readCursor()).toMatchObject({ head: "5", appliedHead: "1" });
  await publish(writer, [], { expectedCursor: "middle", response: response("done", { head: "5" }) });
  expect(await writer.readCursor()).toMatchObject({ head: "5", appliedHead: "5" });
});

it("fences cursor resets against a successor publication and cache replacement", async () => {
  const writer = create();
  await publish(writer, [], { snapshot: true });
  const captured = await writer.capture();
  await publish(writer, [], { expectedCursor: "opaque-cursor", response: response("successor") });
  expect(await writer.reset({ capture: captured, expectedCursor: "opaque-cursor" })).toBe(false);
  expect((await writer.readCursor()).cursor).toBe("successor");
  const retired = await writer.capture();
  await cache.wipeCache();
  await publish(writer, [], { snapshot: true, response: response("replacement") });
  expect(await writer.reset({ capture: retired, expectedCursor: "replacement" })).toBe(false);
  expect((await writer.readCursor()).cursor).toBe("replacement");
  expect(await writer.reset({ capture: await writer.capture(), expectedCursor: "replacement" })).toBe(true);
  expect(await writer.readCursor()).toBeUndefined();
});

it("admits a fenced cursor reset when a first snapshot has no cursor to delete", async () => {
  const writer = create();
  expect(await writer.reset({ capture: await writer.capture() })).toBe(true);
  expect(await writer.readCursor()).toBeUndefined();
});

it("creates the cold legacy query at its exact mounted params", async () => {
  const params = { project_id: projectId, state: "open" };
  const writer = sync.createScopedTaskSyncCache({ deviceId, projectId, scope, bridgeIdentity: deviceId, queryParams: params });
  await publish(writer, [record("task-1")], { snapshot: true });
  expect((await raw(legacy(params))).tasks[0].id).toBe("task-1");
});

it("removes membership on an invalidation with member false", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  await publish(writer, [record("task-1", "2", { operation: "invalidate", member: false, row: null })],
    { expectedCursor: "opaque-cursor", response: response("exit-invalidated") });
  expect(await ids()).toEqual([]);
});

it("invalidates a filter exit without deleting shared task content or other cached memberships", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  const detail = { deviceId, entityId: projectId, kind: "tracker-task", sub: "task-1" };
  const closed = summaries.summaryQueryAddress(place, { states: ["closed"] });
  const inactiveClosed = legacy({ project_id: projectId, state: "closed" });
  const inactiveOpen = legacy({ project_id: projectId, state: "open" });
  await cache.writeCached(detail, { task: task("task-1", { body: "held body" }), timeline: ["held timeline"] });
  await cache.writeCached(closed, { ids: ["task-1"] });
  await cache.writeCached(inactiveClosed, { tasks: [task("task-1")] });
  await cache.writeCached(inactiveOpen, { tasks: [task("task-1")] });
  const closedBefore = await cache.readCached(closed), inactiveBefore = await cache.readCached(inactiveClosed);
  expect(await publish(writer, [record("task-1", "2", { operation: "invalidate", member: false, row: null })],
    { expectedCursor: "opaque-cursor", response: response("filter-exit-invalidated") })).toBe(true);
  expect(await ids()).toEqual([]);
  expect((await raw(inactiveOpen)).tasks).toEqual([]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toMatchObject({ task: { id: "task-1" }, stale: true });
  expect(await raw(detail)).toMatchObject({ task: { id: "task-1", body: "held body" }, timeline: ["held timeline"], stale: true });
  expect((await raw(detail)).task.__task_content_cache.retired.header).toEqual(expect.any(String));
  expect((await cache.readCached(closed)).write).toBe(closedBefore.write);
  expect((await cache.readCached(inactiveClosed)).write).toBe(inactiveBefore.write);
  expect((await raw(closed)).ids).toEqual(["task-1"]);
  expect((await raw(inactiveClosed)).tasks[0].id).toBe("task-1");
  expect(await raw({ deviceId, entityId: projectId, kind: "sync-task-revision", sub: "task-1" })).toMatchObject({ deleted: false, stale: true });
  expect(await writer.readCursor()).toMatchObject({ cursor: "filter-exit-invalidated", snapshotRequired: true });
});

it("publishes live snapshot membership while keeping newer bytes accepted by another scope", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "10", { row: task("task-1", { title: "newer row" }) })], { snapshot: true });
  const closed = create({ ...scope, filters: { states: ["closed"] } });
  expect(await publish(closed, [record("task-1", "9", { row: task("task-1", { title: "older page" }) })], { snapshot: true })).toBe(true);
  expect((await closed.readCursor()).cursor).toBe("opaque-cursor");
  expect(await ids({ states: ["closed"] })).toEqual(["task-1"]);
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.title).toBe("newer row");
  expect((await raw(legacy({ project_id: projectId, states: ["closed"] }))).tasks[0].title).toBe("newer row");
  // Replay after the first-page watermark repairs the historical membership.
  await publish(closed, [record("task-1", "10", { member: false, row: task("task-1", { title: "newer row" }) })],
    { expectedCursor: "opaque-cursor", response: response("replayed") });
  expect(await ids({ states: ["closed"] })).toEqual([]);
});

it("excludes current tombstones from a live snapshot without refusing its other members", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "10", { operation: "delete", row: null, member: false })], { snapshot: true });
  const other = create({ ...scope, filters: { states: ["closed"] } });
  expect(await publish(other, [record("task-1", "9"), record("task-2", "9")], { snapshot: true })).toBe(true);
  expect(await ids({ states: ["closed"] })).toEqual(["task-2"]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
});

it("retains honest normalized uncertainty when a task head lacks unread counter authority", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "1", { row: task("task-1", { watched: true }) })], { snapshot: true });
  expect((await summaries.readSummaryQuery(place, scope.filters)).stale).toBe(true);
});

it("evicts owning project cursors with their replicas so reopening snapshots instead of resuming empty content", async () => {
  const project = create(), detail = create({ kind: "task", task_id: "task-1" });
  const siblingScope = { kind: "project_tasks", project_id: "proj-other", filters: {}, order: "number" };
  const sibling = sync.createScopedTaskSyncCache({ deviceId, projectId: "proj-other", scope: siblingScope, bridgeIdentity: deviceId });
  await publish(project, [record("task-1")], { snapshot: true });
  await publish(detail, [record("task-1", "1", { kind: "task_head", row: { task: task("task-1"), timeline: [] } })], { snapshot: true });
  await publish(sibling, [], { snapshot: true, response: response("sibling-cursor") });
  await cache.evictEntity(deviceId, projectId);
  const reopened = create();
  expect(await reopened.readCursor()).toBeUndefined();
  expect(await create({ kind: "task", task_id: "task-1" }).readCursor()).toBeUndefined();
  expect((await sibling.readCursor()).cursor).toBe("sibling-cursor");
  expect(await publish(reopened, [record("task-1", "2")], { snapshot: true, response: response("reopened-snapshot") })).toBe(true);
  expect(await ids()).toEqual(["task-1"]);
  expect((await reopened.readCursor()).cursor).toBe("reopened-snapshot");
});

it("persists snapshot demand on invalidation through clean batches until snapshot publication", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  expect((await writer.readCursor()).snapshotRequired).toBe(false);
  await publish(writer, [record("task-1", "2", { operation: "invalidate", row: null })],
    { expectedCursor: "opaque-cursor", response: response("invalidated") });
  expect((await writer.readCursor()).snapshotRequired).toBe(true);
  await publish(writer, [], { expectedCursor: "invalidated", response: response("clean") });
  expect((await writer.readCursor()).snapshotRequired).toBe(true);
  await publish(writer, [record("task-1", "3")],
    { snapshot: true, expectedCursor: "clean", response: response("snapshot") });
  expect((await writer.readCursor()).snapshotRequired).toBe(false);
});

it("rolls back an already-issued cursor put when a later row put fails", async () => {
  const writer = create();
  await publish(writer, [record("task-1")], { snapshot: true });
  const captured = await writer.capture();
  const put = IDBObjectStore.prototype.put, written = [];
  const refusal = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(value, key) {
    written.push(String(key));
    if (String(key).includes("|tracker-summary|task-2")) throw new DOMException("late quota", "QuotaExceededError");
    return put.call(this, value, key);
  });
  try {
    expect(await writer.publish({ response: response("should-rollback"), records: [record("task-2", "2")],
      expectedCursor: "opaque-cursor", capture: captured })).toBe(false);
  } finally { refusal.mockRestore(); }
  expect(written.findIndex((key) => key.includes("|sync-cursor|"))).toBeLessThan(written.findIndex((key) => key.includes("|tracker-summary|task-2")));
  expect((await writer.readCursor()).cursor).toBe("opaque-cursor");
  expect(await ids()).toEqual(["task-1"]);
  expect(await raw(summaries.summaryAddress(place, "task-2"))).toBeUndefined();
  expect(await publish(writer, [record("task-2", "2")],
    { expectedCursor: "opaque-cursor", response: response("retry") })).toBe(true);
  expect(await ids()).toEqual(["task-2", "task-1"]);
});

it.each([false, true])("preserves a tombstone against same-revision intra-batch upserts, reversed=%s", async (reversed) => {
  const writer = create();
  await publish(writer, [record("task-1", "1")], { snapshot: true });
  const changes = [record("task-1", "2", { operation: "delete", member: false, row: null }), record("task-1", "2")];
  await publish(writer, reversed ? changes.reverse() : changes,
    { expectedCursor: "opaque-cursor", response: response("same-revision") });
  expect(await ids()).toEqual([]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
});

it("applies server pending-selection metadata when no local mutation is pending", async () => {
  const writer = create();
  expect(await publish(writer, [record("task-1", "1", { selection: { pending_present: true } })], { snapshot: true })).toBe(true);
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.title).toBe("task-1");
  expect((await writer.readCursor()).cursor).toBe("opaque-cursor");
});

it("applies a historical upsert carrying the current tombstone before its later delete event page", async () => {
  const writer = create();
  // Snapshot records have no event sequence, operation, or membership field.
  await publish(writer, [{ kind: "task", key: "task-1", revision: "40", row: task("task-1") }],
    { snapshot: true, response: response("S40", { watermark: "40" }) });
  const detail = { deviceId, entityId: projectId, kind: "tracker-task", sub: "task-1" };
  const inactive = summaries.summaryQueryAddress(place, { states: ["closed"] });
  const inactiveLegacy = legacy({ project_id: projectId, state: "closed" });
  await cache.writeCached(detail, { task: task("task-1", { body: "cached body" }), timeline: ["cached timeline"] });
  await cache.writeCached(inactive, { ids: ["task-1"] });
  await cache.writeCached(inactiveLegacy, { tasks: [task("task-1")] });
  // changes.rs stamps the historical operation onto record()'s current row.
  // The entity was deleted at 43; the page currently replays its upsert at 41.
  const historical = { seq: "41", kind: "task", key: "task-1", operation: "upsert", revision: "43", member: false, row: null };
  expect(await publish(writer, [historical], { expectedCursor: "S40",
    response: response("C41", { head: "43", continuation: "fixed-H43" }) })).toBe(true);
  expect(await ids()).toEqual([]);
  expect((await raw(inactive)).ids).toEqual([]);
  expect((await raw(inactiveLegacy)).tasks).toEqual([]);
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
  expect(await raw(detail)).toBeUndefined();
  expect(await writer.readCursor()).toMatchObject({ cursor: "C41", appliedHead: "40" });
  // A delayed older row cannot resurrect the entity before the delete page.
  await publish(writer, [record("task-1", "42", { seq: "42", operation: "upsert" })],
    { expectedCursor: "C41", response: response("C42", { head: "43", continuation: "fixed-H43-next" }) });
  expect(await raw(summaries.summaryAddress(place, "task-1"))).toBeUndefined();
  expect(await ids()).toEqual([]);
  await publish(writer, [{ ...historical, seq: "43", operation: "delete" }],
    { expectedCursor: "C42", response: response("C43", { head: "43" }) });
  expect(await writer.readCursor()).toMatchObject({ cursor: "C43", appliedHead: "43" });
});

it("populates a cold whole-project replica from an unfiltered default-order snapshot", async () => {
  const writer = create({ kind: "project_tasks", project_id: projectId, filters: {}, order: "number" });
  const whole = { deviceId, entityId: projectId, kind: "tracker-tasks" };
  expect(await raw(whole)).toBeUndefined();
  expect(await publish(writer, [{ kind: "task", key: "task-1", revision: "1", row: task("task-1") }], { snapshot: true })).toBe(true);
  expect((await raw(whole)).tasks.map((task) => task.id)).toEqual(["task-1"]);
  await publish(writer, [record("task-2", "2", { operation: "upsert" })],
    { expectedCursor: "opaque-cursor", response: response("whole-changes") });
  expect((await raw(whole)).tasks.map((task) => task.id)).toEqual(["task-2", "task-1"]);
});

it.each([
  { filters: { states: ["open"] }, order: "number" },
  { filters: {}, order: "priority" },
])("does not create a whole-project replica for a different exact scope %j", async ({ filters, order }) => {
  const writer = create({ kind: "project_tasks", project_id: projectId, filters, order });
  await publish(writer, [{ kind: "task", key: "task-1", revision: "1", row: task("task-1") }], { snapshot: true });
  expect(await raw({ deviceId, entityId: projectId, kind: "tracker-tasks" })).toBeUndefined();
});
