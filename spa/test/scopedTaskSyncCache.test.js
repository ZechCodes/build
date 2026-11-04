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

it("retries a snapshot whose membership row was overtaken by another scope", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "10")], { snapshot: true });
  const closed = create({ ...scope, filters: { states: ["closed"] } });
  expect(await publish(closed, [record("task-1", "9")], { snapshot: true })).toBe(false);
  expect(await closed.readCursor()).toBeUndefined();
  expect((await raw(summaries.summaryAddress(place, "task-1"))).task.id).toBe("task-1");
});

it("retains honest normalized uncertainty when a task head lacks unread counter authority", async () => {
  const writer = create();
  await publish(writer, [record("task-1", "1", { row: task("task-1", { watched: true }) })], { snapshot: true });
  expect((await summaries.readSummaryQuery(place, scope.filters)).stale).toBe(true);
});
