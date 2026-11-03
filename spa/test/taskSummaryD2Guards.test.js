import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

// Five additional public-path controls; the original73 and original22 are
// unchanged. G01 records cursor behavior separately from its authority oracle.
let cache, summaries, readers, content, tracker, order, watch;
let mounted, stops;
const scope = { deviceId: "d2-guards-891", projectId: "proj-1" };
const basis = "opaque-D2-guard-snapshot", firstCursor = "opaque-guard-page-1", secondCursor = "opaque-guard-page-2";
const identity = (id) => ({ deviceId: scope.deviceId, entityId: scope.projectId,
  kind: "tracker-summary-identity", sub: id });

function row(number, extra = {}) {
  return { id: `task-${number}`, project_id: scope.projectId, number, title: `Task ${number}`,
    state: "open", status: "ready", labels: ["api"], priority: "high", assignee: { kind: "user" },
    links: { workspace_ids: [], parent_task_id: null }, trackers: [],
    dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 0 },
    created_by: { kind: "user" }, created_at: "2026-10-29T10:00:00Z",
    updated_at: "2026-10-29T12:00:00Z", closed_at: null, done_at: null,
    watched: true, read_through: "tc-10", watch_started_after: "te-01", dismissed_through: null,
    unread_count: 2, unread_mentions: 1, unread_agent_comments: 1,
    session_started_ms: 1793275200000, last_activity_ms: 1793275200000,
    review_summary: null, deferred_assignment: null, attachment_count: 0, summary_truncated: false, ...extra };
}

function fullTask(number, extra = {}) {
  const task = row(number, { body: `Body ${number}`, attachments: [], ...extra });
  delete task.summary_truncated;
  task.dependencies = { blocked: false, waiting_on: [], blocks_count: 0 };
  return task;
}

const page = (tasks, next_cursor, identities = {}) => ({ project_id: scope.projectId,
  summary: true, basis, tasks, identities, next_cursor });

function controlledReader(filters = {}) {
  let resolveReply, signalDispatch, session = {};
  const reply = new Promise((resolve) => { resolveReply = resolve; });
  const dispatched = new Promise((resolve) => { signalDispatch = resolve; });
  const rpc = vi.fn().mockResolvedValueOnce(page([row(2)], firstCursor, { held: { name: "Held" } }))
    .mockImplementationOnce(() => { signalDispatch(); return reply; });
  const view = readers.createTaskSummaryReader({ scope, filters, callRpc: rpc,
    capabilities: () => ({ listSummaries: true }), sessionOf: () => session });
  mounted.push(view);
  return { view, rpc, dispatched, resolve: resolveReply, replaceSession() { session = {}; } };
}

async function initial(result) {
  expect(await result.view.refresh()).toBe(true);
  expect(await result.view.read()).toMatchObject({ basis, next_cursor: firstCursor, ids: ["task-2"],
    complete: false, stale: false, snapshot_continuation_eligible: true, identities: { held: { name: "Held" } } });
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  expect(result.rpc).toHaveBeenCalledTimes(1);
}

async function pendingMore(result) {
  const pending = result.view.more();
  await result.dispatched;
  expect(result.rpc.mock.calls[1]).toEqual(["tasks.list", { project_id: scope.projectId, summary: true,
    limit: 100, cursor: firstCursor }]);
  return { pending };
}

function protectedAddresses(filters = {}) {
  return [summaries.summaryQueryAddress(scope, filters), summaries.summaryAddress(scope, "task-2"),
    summaries.summaryAddress(scope, "task-1"), identity("held"), identity("new"),
    order.taskContentScopeAddress(scope)];
}

function hearPublications() {
  const heard = [];
  stops.push(cache.subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId }, (change) => heard.push(change)));
  return heard;
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  mounted = []; stops = [];
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  readers = await import("../src/core/taskSummaryReader.js");
  content = await import("../src/core/taskContentAuthority.js");
  tracker = await import("../src/core/trackerCache.js");
  order = await import("../src/core/taskReadOrder.js");
  watch = await import("../src/core/taskWatchAuthority.js");
});

afterEach(() => {
  mounted.forEach((view) => view.dispose()); stops.forEach((stop) => stop()); vi.restoreAllMocks();
});

it("D2-G01 treats a retired-guard empty reply as no progress and preserves its cursor", async () => {
  const result = controlledReader(); await initial(result);
  const { pending } = await pendingMore(result);
  const queryAddress = summaries.summaryQueryAddress(scope, {}), domainAddress = order.taskContentScopeAddress(scope);
  const queryBefore = await cache.readCached(queryAddress), domainBefore = await cache.readCached(domainAddress);
  const rowBefore = await cache.readCached(summaries.summaryAddress(scope, "task-2"));

  // A separate module runtime performs an actual unticketed ordinary write.
  // No invalidation/proof field is inserted or edited by this test.
  vi.resetModules();
  const otherTracker = await import("../src/core/trackerCache.js");
  const detailAddress = tracker.taskAddress(scope.deviceId, scope.projectId, "task-2");
  expect(await otherTracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-2",
    { task: fullTask(2, { title: "Local ordinary edit", body: "Protected local body" }), timeline: ["Local history"] })).toBe(true);
  const domainRetired = await cache.readCached(domainAddress), detailBefore = await cache.readCached(detailAddress);
  expect(domainRetired.value.retirementEpoch).not.toBe(domainBefore.value.retirementEpoch);
  expect(await cache.readCached(queryAddress)).toEqual(queryBefore);
  expect(detailBefore.value.task).toMatchObject({ title: "Local ordinary edit", body: "Protected local body" });
  const watchBefore = await watch.readTaskWatchState(scope);

  result.resolve(page([], secondCursor));
  const moreResult = await pending, after = await result.view.read(), queryAfter = await cache.readCached(queryAddress);
  expect(after).toMatchObject({ snapshot_continuation_eligible: false, stale: true, ids: ["task-2"] });
  expect(queryAfter.value.snapshot_membership?.retirementEpoch).not.toBe(domainRetired.value.retirementEpoch);
  expect(await cache.readCached(detailAddress)).toEqual(detailBefore);
  expect(await cache.readCached(summaries.summaryAddress(scope, "task-2"))).toEqual(rowBefore);
  expect(await watch.readTaskWatchState(scope)).toEqual(watchBefore);
  expect(after).not.toHaveProperty("total");
  expect(moreResult).toBe(false);
  expect(queryAfter).toEqual(queryBefore);
  expect(after.next_cursor).toBe(firstCursor);
  // Retired empty replies cannot consume a cursor or certify exhaustion.
  console.info("D2-G01 cursor observation", JSON.stringify({ moreResult,
    before: { cursor: queryBefore.value.next_cursor, ids: queryBefore.value.ids, generation: queryBefore.value.generation },
    after: { cursor: queryAfter.value.next_cursor, ids: queryAfter.value.ids, generation: queryAfter.value.generation,
      complete: queryAfter.value.complete, snapshot: queryAfter.value.snapshot_membership,
      eligible: after.snapshot_continuation_eligible } }));
});

it("D2-G02 rejects a session replacement inside the real page publication transaction", async () => {
  const result = controlledReader(); await initial(result);
  const { pending } = await pendingMore(result), addresses = protectedAddresses();
  const before = await cache.readCachedMany(addresses), heard = hearPublications();
  const get = IDBObjectStore.prototype.get;
  let replaced = 0;
  const hook = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function(key) {
    const request = get.call(this, key);
    if (String(key).includes("|tracker-summary-query|") && replaced === 0) {
      replaced += 1; result.replaceSession();
    }
    return request;
  });
  result.resolve(page([row(1)], secondCursor, { new: { name: "New" } }));
  expect(await pending).toBe(false);
  hook.mockRestore();
  expect(replaced).toBe(1);
  expect(await cache.readCachedMany(addresses)).toEqual(before);
  expect(await result.view.read()).toMatchObject({ next_cursor: firstCursor, ids: ["task-2"], identities: { held: { name: "Held" } } });
  expect(heard).toEqual([]); expect(result.rpc).toHaveBeenCalledTimes(2);
});

it("D2-G03 rolls back queued snapshot cursor row and identity writes on a late quota abort", async () => {
  const result = controlledReader(); await initial(result);
  const { pending } = await pendingMore(result), addresses = protectedAddresses();
  const before = await cache.readCachedMany(addresses), heard = hearPublications();
  const put = IDBObjectStore.prototype.put, queued = [];
  let failures = 0, aborted = 0, queuedQuery, queryTransaction, abortedTransaction;
  const hook = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(record, key) {
    if (record?.value?.identity?.name === "New" && failures === 0) {
      failures += 1;
      abortedTransaction = this.transaction;
      this.transaction.addEventListener("abort", () => { aborted += 1; });
      throw new DOMException("Late D2 identity quota", "QuotaExceededError");
    }
    const request = put.call(this, record, key);
    queued.push(String(key));
    if (String(key).includes("|tracker-summary-query|")) {
      queuedQuery = record.value; queryTransaction = this.transaction;
    }
    return request;
  });
  result.resolve(page([row(1)], secondCursor, { new: { name: "New" } }));
  expect(await pending).toBe(false);
  hook.mockRestore();
  expect(failures).toBe(1); expect(aborted).toBe(1);
  expect(queryTransaction).toBe(abortedTransaction);
  expect(queuedQuery).toMatchObject({ ids: ["task-2", "task-1"], next_cursor: secondCursor,
    snapshot_membership: { ids: ["task-2", "task-1"], basis, next_cursor: secondCursor } });
  expect(queued.some((key) => key.includes("|tracker-summary-query|"))).toBe(true);
  expect(queued.some((key) => key.endsWith("|tracker-summary|task-1"))).toBe(true);
  expect(await cache.readCachedMany(addresses)).toEqual(before);
  expect(await result.view.read()).toMatchObject({ next_cursor: firstCursor, ids: ["task-2"],
    snapshot_continuation_eligible: true, identities: { held: { name: "Held" } } });
  expect(heard).toEqual([]); expect(result.rpc).toHaveBeenCalledTimes(2);
});

it("D2-G04 continues a raw Ready snapshot without certifying its retained future Done row", async () => {
  const detailAddress = tracker.taskAddress(scope.deviceId, scope.projectId, "task-2");
  const held = fullTask(2, { title: "Future Done header", body: "Future Done body", status: "done",
    done_at: "2027-10-29T11:00:00Z", updated_at: "2027-10-29T12:00:00Z" });
  const contentRead = await content.captureTaskContentRead(detailAddress, { revalidate: "full" });
  expect(await tracker.writeTaskRecord(scope.deviceId, scope.projectId, "task-2", { task: held, timeline: ["Future history"] },
    { contentRead })).toBe(true);
  const detailBefore = await cache.readCached(detailAddress);
  const result = controlledReader({ statuses: ["ready"] });
  expect(await result.view.refresh()).toBe(true);
  const mixed = await result.view.read(), normalizedAddress = summaries.summaryAddress(scope, "task-2");
  expect(mixed).toMatchObject({ ids: ["task-2"], stale: true, snapshot_continuation_eligible: true,
    tasks: [{ status: "done", title: "Future Done header" }] });
  const normalizedBefore = await cache.readCached(normalizedAddress);
  expect(normalizedBefore.value).toMatchObject({ basis_uncertain: true, watch_uncertain: false });
  expect(normalizedBefore.value.basis).toBeUndefined();
  const pending = result.view.more(); await result.dispatched;
  result.resolve(page([], secondCursor)); expect(await pending).toBe(true);
  expect(result.rpc).toHaveBeenCalledTimes(2);
  expect(result.rpc.mock.calls[1][1]).toMatchObject({ statuses: ["ready"], cursor: firstCursor });
  expect(await result.view.read()).toMatchObject({ ids: ["task-2"], next_cursor: secondCursor, stale: true,
    snapshot_continuation_eligible: true, tasks: [{ status: "done", title: "Future Done header" }] });
  expect(await cache.readCached(normalizedAddress)).toEqual(normalizedBefore);
  expect(await cache.readCached(detailAddress)).toEqual(detailBefore);
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
});

it("D2-G05 continues with a missing display row while preserving raw membership and unknown absence", async () => {
  const result = controlledReader(); await initial(result);
  const normalizedAddress = summaries.summaryAddress(scope, "task-2");
  await cache.deleteCached([normalizedAddress]);
  expect(await result.view.read()).toMatchObject({ ids: ["task-2"], missing_ids: ["task-2"], tasks: [],
    stale: true, complete: false, snapshot_continuation_eligible: true });
  const { pending } = await pendingMore(result);
  result.resolve(page([], secondCursor)); expect(await pending).toBe(true);
  const after = await result.view.read();
  expect(after).toMatchObject({ ids: ["task-2"], missing_ids: ["task-2"], tasks: [], next_cursor: secondCursor,
    stale: true, complete: false, snapshot_continuation_eligible: true });
  const saved = (await cache.readCached(summaries.summaryQueryAddress(scope, {}))).value;
  expect(saved).toMatchObject({ ids: ["task-2"], complete: false,
    snapshot_membership: { ids: ["task-2"], basis, next_cursor: secondCursor, complete: false } });
  expect(await cache.readCached(normalizedAddress)).toBeUndefined();
  expect(after).not.toHaveProperty("total");
  const { referenceTables, resolverFor } = await import("../src/core/referenceTargets.js");
  const { deviceKey } = await import("../src/core/deviceKey.js");
  const key = deviceKey(scope.deviceId, scope.projectId);
  expect(resolverFor(referenceTables({ tasks: { [key]: after.tasks } }), { place: scope }).task(2)).toBeUndefined();
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: false, eligible: true });
  expect(result.rpc).toHaveBeenCalledTimes(2);
});
