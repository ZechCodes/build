// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task as wireTask } from "./trackerWireFixture.js";

// Proposed retired-page contract, approved by lead4576. These NEW controls do
// not alter the original73/StageA5 or their recorded cursor observation.
// DOM coverage uses the actual production row renderer over public reader
// cache output. No production D2 consumer is wired or invented by this harness.
let cache, summaries, readers, tracker, order, watch, projection, paintTaskRows;
let mounted;
const scope = { deviceId: "d2-retired-page-891", projectId: "proj-1" };
const oldBasis = "opaque-before-retirement", newBasis = "opaque-current-snapshot";
const oldCursor = "opaque-old-cursor", rejectedCursor = "opaque-rejected-cursor";
const queryAddress = () => summaries.summaryQueryAddress(scope, {});
const detailAddress = () => tracker.taskAddress(scope.deviceId, scope.projectId, "task-2");
const normalizedAddress = () => summaries.summaryAddress(scope, "task-2");
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };

function full(number = 2, extra = {}) {
  return wireTask({ id: `task-${number}`, project_id: scope.projectId, number, title: `Task ${number}`,
    body: `Body ${number}`, status: "ready", assignee: { kind: "user" }, labels: ["api"], watched: true,
    read_through: "tc-10", watch_started_after: "te-01", dismissed_through: "te-05",
    unread_count: 2, unread_mentions: 1, unread_agent_comments: 1,
    review_summary: { task_id: `task-${number}`, version: 1, mode: "pull_request", status: "open" },
    updated_at: "2026-10-29T12:00:00Z", attachments: [], ...extra });
}
const row = (number = 2, extra = {}) => ({ ...projection.fullTaskSummary(full(number, extra)), summary_truncated: false });
const page = (tasks, basis = oldBasis, next_cursor = oldCursor, identities = {}) =>
  ({ project_id: scope.projectId, summary: true, tasks, basis, next_cursor, identities });

function controlledReader() {
  const replies = [deferred(), deferred(), deferred()];
  const dispatched = replies.map(() => deferred());
  const rpc = vi.fn().mockResolvedValueOnce(page([row()], oldBasis, oldCursor, { held: { name: "Held" } }));
  for (const [index, reply] of replies.entries()) rpc.mockImplementationOnce(() => {
    dispatched[index].resolve(); return reply.promise;
  });
  const session = {};
  const view = readers.createTaskSummaryReader({ scope, filters: {}, callRpc: rpc,
    capabilities: () => ({ listSummaries: true }), sessionOf: () => session });
  mounted.push(view);
  return { view, rpc, replies, dispatched };
}

async function initial(result) {
  expect(await result.view.refresh()).toBe(true);
  expect(await result.view.read()).toMatchObject({ ids: ["task-2"], basis: oldBasis, next_cursor: oldCursor,
    stale: false, complete: false, snapshot_continuation_eligible: true });
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
}

async function retireOrdinary(sequence) {
  const before = await cache.readCached(order.taskContentScopeAddress(scope));
  vi.resetModules();
  const other = await import("../src/core/trackerCache.js");
  expect(await other.writeTaskRecord(scope.deviceId, scope.projectId, "task-2", {
    task: full(2, { title: `Local edit ${sequence}`, body: `Protected local body ${sequence}` }), timeline: ["Local history"],
  })).toBe(true);
  const after = await cache.readCached(order.taskContentScopeAddress(scope));
  expect(after.value.retirementEpoch).not.toBe(before.value.retirementEpoch);
}

async function queuedRetiredContinuation(result) {
  const pending = result.view.more(); await result.dispatched[0].promise;
  expect(result.rpc.mock.calls[1][1]).toEqual({ project_id: scope.projectId, summary: true, limit: 100, cursor: oldCursor });
  await retireOrdinary(1);
  return { pending, queryBefore: await cache.readCached(queryAddress()), detailBefore: await cache.readCached(detailAddress()) };
}

async function draw(view, body) {
  const result = await view.read();
  paintTaskRows(body, result.tasks, { columns: columns(), href: (task) => `#/tasks/${task.id}`,
    nowMs: Date.parse("2026-10-29T12:00:00Z"), agentLabels: {} }, () => {});
  return result;
}

beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange; mounted = [];
  document.body.innerHTML = '<div id="d2-retired-rows"></div>';
  cache = await import("../src/core/localCache.js"); summaries = await import("../src/core/taskSummaryCache.js");
  readers = await import("../src/core/taskSummaryReader.js"); tracker = await import("../src/core/trackerCache.js");
  order = await import("../src/core/taskReadOrder.js"); watch = await import("../src/core/taskWatchAuthority.js");
  projection = await import("../src/core/taskSummaryProjection.js");
  ({ paintTaskRows } = await import("../src/core/trackerTasksBody.js"));
});
afterEach(() => { mounted.forEach((view) => view.dispose()); vi.restoreAllMocks(); });

it("D2-RP01 preserves the exact current query and actual row paint when a retired empty page replies", async () => {
  const result = controlledReader(); await initial(result);
  const body = document.querySelector("#d2-retired-rows"); await draw(result.view, body);
  const element = body.querySelector('[data-task="task-2"]'), html = body.innerHTML;
  expect(body.querySelector(".task-title").textContent).toBe("Task 2");
  expect(body.querySelector(".task-unread").textContent).toBe("2");
  const { pending, queryBefore, detailBefore } = await queuedRetiredContinuation(result);
  const rowBefore = await cache.readCached(normalizedAddress());
  result.replies[0].resolve(page([], oldBasis, rejectedCursor));
  const admitted = await pending; await draw(result.view, body);
  // The new failing-first oracle: the entire current query record (including
  // write stamp/metadata) stays untouched, not only its visible ID prefix.
  expect(await cache.readCached(queryAddress())).toEqual(queryBefore);
  expect(admitted).toBe(false);
  expect(body.innerHTML).toBe(html); expect(body.querySelector('[data-task="task-2"]')).toBe(element);
  expect(await cache.readCached(normalizedAddress())).toEqual(rowBefore);
  expect(await cache.readCached(detailAddress())).toEqual(detailBefore);
  expect(await result.view.read()).toMatchObject({ ids: ["task-2"], next_cursor: oldCursor,
    stale: true, snapshot_continuation_eligible: false });
  expect(result.rpc).toHaveBeenCalledTimes(2);
});

it("D2-RP02 preserves retired query membership while independently admitting same-task marks and PR version", async () => {
  const result = controlledReader(); await initial(result);
  const { pending, queryBefore, detailBefore } = await queuedRetiredContinuation(result);
  result.replies[0].resolve(page([row(2, { title: "Rejected ordinary wire header", read_through: "tc-20",
    dismissed_through: "te-20", unread_count: 1, unread_mentions: 0, unread_agent_comments: 1,
    review_summary: { task_id: "task-2", version: 2, mode: "pull_request", status: "approved" } })],
  oldBasis, rejectedCursor, { independent: { name: "Independent actor" } }));
  const admitted = await pending, selected = (await cache.readCached(normalizedAddress())).value;
  // Check the independently admitted families before the refusal assertion:
  // an early return that drops every row is not the requested repair.
  expect(selected.task).toMatchObject({ id: "task-2", title: "Task 2", read_through: "tc-20",
    dismissed_through: "te-20", review_summary: { task_id: "task-2", version: 2, status: "approved" } });
  expect(selected).toMatchObject({ watch_uncertain: false, basis_uncertain: true });
  expect(selected.basis).toBeUndefined();
  expect(await cache.readCached(detailAddress())).toEqual(detailBefore);
  expect(await cache.readCached(queryAddress())).toEqual(queryBefore);
  expect(admitted).toBe(false);
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ uncertain: false, eligible: true });
  expect(result.rpc).toHaveBeenCalledTimes(2);
});

it("D2-RP03 restarts at page one on the next user more and coalesces overlapping refresh demand", async () => {
  const result = controlledReader(); await initial(result);
  const { pending } = await queuedRetiredContinuation(result);
  result.replies[0].resolve(page([], oldBasis, rejectedCursor)); await pending;
  expect(result.rpc).toHaveBeenCalledTimes(2);
  const body = document.querySelector("#d2-retired-rows"); await draw(result.view, body); const html = body.innerHTML;
  const user = result.view.more(), duplicate = result.view.more();
  const firstEffect = await Promise.race([result.dispatched[1].promise.then(() => "dispatched"),
    user.then((value) => ({ settledWithoutDispatch: value }))]);
  expect(firstEffect).toBe("dispatched");
  // Background demand arrives only AFTER the user's own first-page dispatch;
  // it cannot mask a broken user-more restart by starting the request itself.
  const background = result.view.refresh();
  expect(result.rpc).toHaveBeenCalledTimes(3);
  expect(result.rpc.mock.calls[2]).toEqual(["tasks.list", { project_id: scope.projectId, summary: true, limit: 100 }]);
  await draw(result.view, body); expect(body.innerHTML).toBe(html);
  result.replies[1].resolve(page([row(3), row(2, { title: "Current server header", updated_at: "2026-10-29T13:00:00Z" })], newBasis, null));
  expect(await Promise.all([user, duplicate, background])).toEqual([true, true, true]);
  expect(await result.view.read()).toMatchObject({ ids: ["task-3", "task-2"], basis: newBasis,
    next_cursor: null, complete: true, stale: false, tasks: [{ id: "task-3" }, { id: "task-2", title: "Current server header" }] });
  expect(result.rpc).toHaveBeenCalledTimes(3);
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ uncertain: false, eligible: true });
});

it("D2-RP04 repeated retirement preserves each current query without an automatic restart loop", async () => {
  const result = controlledReader(); await initial(result);
  const { pending } = await queuedRetiredContinuation(result);
  result.replies[0].resolve(page([], oldBasis, rejectedCursor)); await pending;
  // A normal refresh starts a fresh page-one request. Retire that request too;
  // refusal must not auto-drain/restart or clear its retained cached paint.
  const refresh = result.view.refresh(); await result.dispatched[1].promise;
  await retireOrdinary(2);
  const before = await cache.readCached(queryAddress()), body = document.querySelector("#d2-retired-rows");
  await draw(result.view, body); const html = body.innerHTML;
  result.replies[1].resolve(page([], newBasis, "opaque-also-rejected"));
  const admitted = await refresh; await draw(result.view, body);
  expect(await cache.readCached(queryAddress())).toEqual(before); expect(admitted).toBe(false);
  expect(body.innerHTML).toBe(html); expect(result.rpc).toHaveBeenCalledTimes(3);
  await result.view.read(); expect(result.rpc).toHaveBeenCalledTimes(3);
  const user = result.view.more(), duplicate = result.view.more();
  const firstEffect = await Promise.race([result.dispatched[2].promise.then(() => "dispatched"),
    user.then((value) => ({ settledWithoutDispatch: value }))]);
  expect(firstEffect).toBe("dispatched");
  expect(result.rpc).toHaveBeenCalledTimes(4);
  expect(result.rpc.mock.calls[3][1]).toEqual({ project_id: scope.projectId, summary: true, limit: 100 });
  await retireOrdinary(3); const last = await cache.readCached(queryAddress());
  result.replies[2].resolve(page([], "opaque-third-retired-basis", "opaque-third-rejected-cursor"));
  expect(await Promise.all([user, duplicate])).toEqual([false, false]);
  expect(await cache.readCached(queryAddress())).toEqual(last);
  await draw(result.view, body); expect(body.innerHTML).toBe(html);
  expect(result.rpc).toHaveBeenCalledTimes(4);
  await result.view.read(); expect(result.rpc).toHaveBeenCalledTimes(4);
});
