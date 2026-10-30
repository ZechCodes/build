import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const scope = { deviceId: "shared-watch-device", projectId: "proj-1" };
let runtimes;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  runtimes = [];
});

afterEach(() => {
  for (const runtime of runtimes) runtime.db.close();
});

// Resetting the import graph leaves earlier module instances alive. Each owns
// its actual database connection, allocator module, cache announcement state,
// and local mutation guard while all connections see the same backing store.
async function independentRuntime() {
  vi.resetModules();
  const [cache, order, content, tracker, summaries, actions, mutation, watch] = await Promise.all([
    import("../src/core/localCache.js"),
    import("../src/core/taskReadOrder.js"),
    import("../src/core/taskContentAuthority.js"),
    import("../src/core/trackerCache.js"),
    import("../src/core/taskSummaryCache.js"),
    import("../src/core/taskWaitingOnCache.js"),
    import("../src/core/taskMutationFence.js"),
    import("../src/core/taskWatchAuthority.js"),
  ]);
  let db;
  const opened = await cache.transactWriteOperations("readonly", ({ records }) => {
    db = records.transaction.db;
    return null;
  });
  expect(opened.committed).toBe(true);
  for (const prior of runtimes) {
    expect(cache).not.toBe(prior.cache);
    expect(order).not.toBe(prior.order);
    expect(content).not.toBe(prior.content);
    expect(mutation).not.toBe(prior.mutation);
    expect(watch).not.toBe(prior.watch);
    expect(db).not.toBe(prior.db);
    expect(db.name).toBe(prior.db.name);
  }
  const runtime = { cache, order, content, tracker, summaries, actions, mutation, watch, db };
  runtimes.push(runtime);
  return runtime;
}

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

const taskId = "shared-watch-task";
const fullTask = (over = {}) => task({ id: taskId, project_id: scope.projectId,
  attachments: [], watched: true, unread_count: 2, read_through: "tc-10", ...over });
const stateOf = (runtime) => runtime.watch.readTaskWatchState(scope);
const begin = (runtime, options = {}) => runtime.watch.beginTaskWatchMutation(scope, taskId,
  { watched: false, metadataOnly: true, ...options });
const rowOf = async (runtime) => (await runtime.tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];

async function seedDisplay(runtime) {
  // A legacy display row is deliberately not a known-zero coordination seed.
  // These controls test owner cleanup and optimism, never baseline recovery.
  await runtime.cache.writeCached(runtime.tracker.tasksAddress(scope.deviceId, scope.projectId),
    runtime.tracker.tasksRecord([fullTask()], []));
}

it("shares overlapping owners across independent connections while local runtime guards remain independent", async () => {
  const first = await independentRuntime(), outside = await independentRuntime();
  const rpc = deferred();
  const pendingCall = first.mutation.wrapTaskMutationCall(scope.deviceId, "tasks.unwatch", () => rpc.promise);
  try {
    expect(first.mutation.taskMutationsPending(scope.deviceId)).toBe(1);
    expect(outside.mutation.taskMutationsPending(scope.deviceId)).toBe(0);
    const older = await begin(first), newer = await begin(outside, { watched: true });
    expect(older).not.toBeNull(); expect(newer).not.toBeNull();
    expect(newer.intentRead).toBeGreaterThan(older.intentRead);
    expect(await stateOf(first)).toMatchObject({ pendingCount: 2, uncertain: true, eligible: false });
    expect(await stateOf(outside)).toEqual(await stateOf(first));
    const during = await outside.content.captureTaskContentRead(scope);
    expect(during.ticket).toMatchObject({ watchPending: true, watchUncertain: true });

    // The serializable exact owner works outside its initiating runtime. Its
    // obsolete task intent may clean up itself without rolling back the newer one.
    expect(await outside.watch.settleTaskWatchMutation(JSON.parse(JSON.stringify(older)), { rollback: true })).toBe(false);
    const remaining = await stateOf(first);
    expect(remaining).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
    expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
    expect(await stateOf(outside)).toEqual(remaining);
    expect(await first.watch.settleTaskWatchMutation(newer, { rollback: true })).toBe(true);
    expect(await stateOf(outside)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  } finally {
    rpc.resolve({}); await pendingCall;
  }
});

it("rolls back only the latest matching optimistic owner across connections", async () => {
  const first = await independentRuntime(), outside = await independentRuntime();
  await seedDisplay(first);
  const older = await begin(first, { metadataOnly: false });
  expect((await rowOf(outside)).watched).toBe(false);
  const newer = await begin(outside, { watched: true, metadataOnly: false });
  expect((await rowOf(first)).watched).toBe(true);
  expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  expect((await rowOf(outside)).watched).toBe(true);
  expect(await stateOf(outside)).toMatchObject({ pendingCount: 1, uncertain: true });
  expect(await first.watch.settleTaskWatchMutation(newer, { rollback: true })).toBe(true);
  expect((await rowOf(outside)).watched).toBe(false);
  expect(await stateOf(outside)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  expect((await rowOf(outside))[outside.watch.TASK_WATCH_CACHE]).toBeUndefined();
});

it("retains orphan uncertainty after the initiating connection closes and another runtime reopens", async () => {
  const first = await independentRuntime();
  const owner = await begin(first);
  expect(owner).not.toBeNull();
  const before = await stateOf(first);
  first.db.close();
  const reopened = await independentRuntime();
  expect(reopened.mutation.taskMutationsPending(scope.deviceId)).toBe(0);
  expect(await stateOf(reopened)).toEqual(before);
  // Reads and another local owner's complete lifecycle cannot recover an orphan.
  await reopened.content.captureTaskContentRead(scope);
  const next = await begin(reopened);
  await reopened.watch.settleTaskWatchMutation(next, { rollback: true });
  expect(await stateOf(reopened)).toMatchObject({ pendingCount: 1,
    pendingGeneration: before.pendingGeneration, uncertain: true, eligible: false });
});

it("keeps coordination across project eviction and prevents stale settlement from touching the new owner", async () => {
  const first = await independentRuntime(), outside = await independentRuntime();
  await seedDisplay(first);
  const older = await begin(first, { metadataOnly: false });
  const before = await stateOf(outside);
  await outside.cache.evictEntity(scope.deviceId, scope.projectId);
  expect(await stateOf(first)).toEqual(before);
  await seedDisplay(outside);
  const newer = await begin(outside, { metadataOnly: false });
  expect(newer.scopeGeneration).not.toBe(older.scopeGeneration);
  expect(await stateOf(first)).toMatchObject({ pendingCount: 2 });
  expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  expect((await rowOf(outside)).watched).toBe(false);
  const remaining = await stateOf(outside);
  expect(remaining).toMatchObject({ pendingCount: 1, uncertain: true });
  expect(await outside.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  expect(await stateOf(first)).toEqual(remaining);
  expect(await outside.watch.settleTaskWatchMutation(newer, { rollback: true })).toBe(true);
  expect((await rowOf(first)).watched).toBe(true);
});

it("fails closed after a real reset and rejects the old generation without clearing its replacement", async () => {
  const first = await independentRuntime(), outside = await independentRuntime();
  const older = await begin(first);
  await outside.cache.wipeCache();
  expect(await stateOf(first)).toMatchObject({ pendingCount: null, pendingGeneration: null, uncertain: true, eligible: false });
  const newer = await begin(outside);
  expect(newer.pendingGeneration).not.toBe(older.pendingGeneration);
  expect(newer.storeGeneration).not.toBe(older.storeGeneration);
  const replacement = await stateOf(outside);
  expect(replacement).toMatchObject({ pendingCount: 1, uncertain: true });
  expect(await first.watch.settleTaskWatchMutation(older, { rollback: true })).toBe(false);
  expect(await stateOf(outside)).toEqual(replacement);
  expect(await first.watch.settleTaskWatchMutation(newer, { rollback: true })).toBe(true);
  expect(await stateOf(outside)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
});

it.each(["fresh", "reset"])("does not launder %s uncertainty through its own successful server answer", async (kind) => {
  const first = await independentRuntime(), outside = await independentRuntime();
  if (kind === "reset") {
    await begin(first);
    await outside.cache.wipeCache();
  }
  expect(await stateOf(first)).toMatchObject({ pendingCount: null, uncertain: true, eligible: false });
  const owner = await begin(first);
  const contentRead = await first.content.captureTaskContentRead(scope);
  expect(await stateOf(outside)).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
  // A real full answer acknowledges this owner, but cannot recover an unknown
  // baseline. Cleanup and eligibility are deliberately separate assertions.
  await outside.watch.settleTaskWatchMutation(owner, { answer: { task: fullTask({ watched: false }) }, contentRead });
  expect(await stateOf(first)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  const fresh = await outside.content.captureTaskContentRead(scope);
  expect(fresh.ticket.watchUncertain).toBe(true);
  expect(fresh.ticket.watchPending).toBe(false);
});

it.each(["taskId", "controlGeneration"])("rejects a copied owner with altered %s across connections", async (field) => {
  const first = await independentRuntime(), outside = await independentRuntime();
  const owner = await begin(first);
  const pending = await stateOf(outside);
  const altered = { ...JSON.parse(JSON.stringify(owner)), [field]: "different-owner-field" };
  expect(await outside.watch.settleTaskWatchMutation(altered, { rollback: true })).toBe(false);
  expect(await stateOf(first)).toEqual(pending);
  expect(await outside.watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
  expect(await stateOf(first)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
});
