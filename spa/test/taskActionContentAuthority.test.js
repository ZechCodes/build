import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import taskViewFixture from "../../fixtures/api/v1/tasks.read_through.json";

let cache, tracker, summaries, actions, order, pages, projection;
const scope = { deviceId: "action-content-900", projectId: "proj-1" };
const taskId = taskViewFixture.result.task.id, high = "2026-10-29T12:00:00.000900Z", low = "2026-10-29T12:00:00.000100Z";
const full = (extra = {}) => ({ ...structuredClone(taskViewFixture.result.task), project_id: scope.projectId,
  watched: false, title: "Current ordinary title", body: "Current full body", status: "ready", updated_at: high, ...extra });
const timeline = [{ id: "te-01K5ZQ8M4T0J7WQ2R6X3YB9C51", type: "event", task_id: taskId, kind: "moved",
  actor: { kind: "user" }, payload: { from: "backlog", to: "ready" }, created_at: high }];
const value = async (address) => (await cache.readCached(address))?.value;
const lists = () => [tracker.tasksAddress(scope.deviceId, scope.projectId),
  tracker.tasksQueryAddress(scope.deviceId, scope.projectId, { state: "open" }),
  tracker.tasksPageAddress(scope.deviceId, scope.projectId, { limit: 100 })];
const detailAddress = () => tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
const snapshotAddresses = () => [detailAddress(), ...lists(), ...lists().map(order.readsAddress),
  order.readsAddress(detailAddress()), summaries.summaryAddress(scope, taskId)];
async function installOrdinaryRead(task, read) {
  const detailBasis = await tracker.captureTaskDetailRead(scope.deviceId, scope.projectId, taskId);
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    tracker.taskRecord(task, timeline), { detailBasis })).toBe(true);
  for (const address of lists()) await pages.foldTasksPage(address, {
    tasks: [task], read, above: Infinity, through: -Infinity,
  }, () => []);
  const ticket = await summaries.beginSummaryQuery(scope, {}, read);
  expect(await summaries.landSummaryPage(ticket, { summary: true, basis: `snapshot-${read}`,
    tasks: [projection.summaryPatch(undefined, task, true)] }, { read })).toBe(true);
}
async function arrange(olderAt) {
  await installOrdinaryRead(full({ title: "Initial title", body: "Initial body", updated_at: "2026-10-29T11:59:00Z" }), await order.nextTaskRead());
  const issuedRead = await order.nextTaskRead(), newerRead = await order.nextTaskRead();
  await installOrdinaryRead(full(), newerRead);
  // Actual list folds and the normalized page retain their asked-at order.
  for (const address of lists()) expect(order.lastSayIn(await value(order.readsAddress(address)))(full())).toBe(newerRead);
  expect((await value(summaries.summaryAddress(scope, taskId))).read_order).toBe(newerRead);
  // Detail pulls currently persist no own dispatch clock; do not invent one.
  expect(await tracker.captureTaskDetailRead(scope.deviceId, scope.projectId, taskId)).toBe(0);
  expect(issuedRead).toBeLessThan(newerRead);
  return { issuedRead, answer: { task: full({ title: "Older action title", body: "Older action body", status: "in_progress", updated_at: olderAt }) } };
}
async function expectCurrentOrdinaryContent() {
  const detail = await tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  const rows = [detail.task, ...await Promise.all(lists().map(async (address) => (await value(address)).tasks[0])),
    (await value(summaries.summaryAddress(scope, taskId))).task];
  expect({ rows: rows.map(({ title, status, updated_at }) => ({ title, status, updated_at })),
    body: detail.task.body, timeline: detail.timeline, summaryHasBody: Object.hasOwn(rows.at(-1), "body") }).toEqual({
    rows: rows.map(() => ({ title: "Current ordinary title", status: "ready", updated_at: high })),
    body: "Current full body", timeline, summaryHasBody: false,
  });
}
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js"); tracker = await import("../src/core/trackerCache.js");
  summaries = await import("../src/core/taskSummaryCache.js"); actions = await import("../src/core/taskWaitingOnCache.js");
  order = await import("../src/core/taskReadOrder.js"); pages = await import("../src/core/trackerPages.js");
  projection = await import("../src/core/taskSummaryProjection.js");
});

it.each([["native sub-millisecond", low], ["equal timestamp", high]])(
  "retains newer-issued ordinary content over an older full action with %s timestamps", async (_label, olderAt) => {
    expect(Date.parse(olderAt)).toBe(Date.parse(high));
    const { issuedRead, answer } = await arrange(olderAt);
    await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, answer, { issuedRead });
    await expectCurrentOrdinaryContent();
  },
);

it("keeps the captured-session replacement control green while full-action publication waits for its transaction", async () => {
  const { issuedRead, answer } = await arrange(low), capturedSession = {}; let session = capturedSession;
  const before = await Promise.all(snapshotAddresses().map(value)), original = IDBObjectStore.prototype.get;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    if (String(key).includes("|tracker-summary|")) session = {};
    return original.call(this, key);
  });
  expect(await actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, answer,
    { issuedRead, accept: () => session === capturedSession })).toBe(false);
  spy.mockRestore(); expect(session).not.toBe(capturedSession);
  expect(await Promise.all(snapshotAddresses().map(value))).toEqual(before);
  await expectCurrentOrdinaryContent();
});
