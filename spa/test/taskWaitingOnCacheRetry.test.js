import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const scope = { deviceId: "action-retry-device", projectId: "action-retry-project" };
const taskId = "action-retry-task";
const recovery = { reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40 };
let runtimes;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  runtimes = [];
});

afterEach(async () => {
  vi.restoreAllMocks();
  for (const runtime of runtimes) {
    let db;
    await runtime.cache.transactWriteOperations("readonly", ({ records }) => { db = records.transaction.db; });
    db?.close();
  }
});

async function independentRuntime() {
  vi.resetModules();
  const [cache, tracker, content, actions, watch, keys, summaries] = await Promise.all([
    import("../src/core/localCache.js"),
    import("../src/core/trackerCache.js"),
    import("../src/core/taskContentAuthority.js"),
    import("../src/core/taskWaitingOnCache.js"),
    import("../src/core/taskWatchAuthority.js"),
    import("../src/core/idbRecords.js"),
    import("../src/core/taskSummaryCache.js"),
  ]);
  cache.setCacheRecoveryTiming(recovery);
  let db;
  const opened = await cache.transactWriteOperations("readonly", ({ records }) => { db = records.transaction.db; });
  expect(opened.committed).toBe(true);
  for (const prior of runtimes) {
    expect(cache).not.toBe(prior.cache);
    expect(content).not.toBe(prior.content);
    expect(db).not.toBe(prior.db);
  }
  const runtime = { cache, tracker, content, actions, watch, keys, summaries, db };
  runtimes.push(runtime);
  return runtime;
}

async function prepareAction() {
  const writer = await independentRuntime(), observer = await independentRuntime();
  const held = task({ id: taskId, project_id: scope.projectId, title: "Before the action", body: "Before body", watched: false });
  const catalogue = writer.tracker.tasksAddress(scope.deviceId, scope.projectId);
  const catalogueRead = await writer.content.captureTaskContentRead(catalogue, { revalidate: "full" });
  await writer.tracker.writeTasksRecord(scope.deviceId, scope.projectId, writer.tracker.tasksRecord([
    writer.content.observeTaskContent(held, { contentRead: catalogueRead, coverage: "full" }),
  ], [], catalogueRead.readOrder), { contentRead: catalogueRead });
  const detail = writer.tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const detailRead = await writer.content.captureTaskContentRead(detail, { revalidate: "full" });
  await writer.tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    writer.tracker.taskRecord(held, [{ id: "before-event", kind: "created" }]), { contentRead: detailRead });
  const owner = await writer.watch.beginTaskWatchMutation(scope, taskId, { watched: true });
  expect(owner).not.toBeNull();
  expect(await observer.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1 });
  const contentRead = await writer.content.captureTaskContentRead(scope);
  expect(contentRead.ticket).not.toBeNull();
  expect(contentRead.readOrder).toBeGreaterThan(owner.intentRead);
  const answer = { task: { ...held, title: "Response data", body: "Response body", watched: true,
    updated_at: "2026-10-30T00:00:00Z" }, timeline: [{ id: "response-event", kind: "updated" }] };
  return { writer, observer, owner, contentRead, answer, detail, catalogue };
}

function abortFirstResponsePut(runtime, detail, retire = () => {}) {
  const originalPut = IDBObjectStore.prototype.put;
  const detailKey = runtime.keys.recordKey(detail), attempts = [];
  const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (key === detailKey && record?.value?.task?.title === "Response data") {
      // This is a value produced by the real action reducer, after its
      // publication decision, rather than an allocator or setup write.
      attempts.push(structuredClone(record.value));
      if (attempts.length === 1) {
        retire();
        throw new DOMException("connection closed during action publication", "InvalidStateError");
      }
    }
    return originalPut.call(this, record, key);
  });
  return { attempts, put };
}

it("returns false when recovery commits only owner cleanup after the captured session retires", async () => {
  const { writer, observer, owner, contentRead, answer, detail, catalogue } = await prepareAction();
  const summary = writer.summaries.summaryAddress(scope, taskId);
  const beforeDetail = await observer.cache.readCached(detail), beforeSummary = await observer.cache.readCached(summary);
  const beforeCatalogue = (await observer.cache.readCached(catalogue)).value;
  expect(beforeCatalogue.tasks[0].__task_watch_intent).toBe(owner.id);
  expect(beforeCatalogue.tasks[0].__task_watch_intent_cache.ownerId).toBe(owner.id);
  const publications = [];
  const stopDetail = writer.cache.subscribeCache(detail, (address) => publications.push(address));
  const stopSummary = writer.cache.subscribeCache(summary, (address) => publications.push(address));
  const capturedSession = {}, replacementSession = {};
  let currentSession = capturedSession;
  const failure = abortFirstResponsePut(writer, detail, () => { currentSession = replacementSession; });
  const published = await writer.actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, answer,
    { watchOwner: owner, contentRead, issuedRead: contentRead.readOrder, accept: () => currentSession === capturedSession });
  stopDetail(); stopSummary();
  expect(failure.attempts).toHaveLength(1);
  expect(failure.attempts[0].task).toMatchObject({ title: answer.task.title, body: answer.task.body, watched: true });
  expect(writer.cache.cacheHealth().state).toBe("ready");
  expect(await observer.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0 });
  expect(await observer.cache.cachedAddresses({ deviceId: scope.deviceId, entityId: "", kind: "task-watch-owner" })).toEqual([]);
  expect(await observer.cache.readCached(detail)).toEqual(beforeDetail);
  expect(await observer.cache.readCached(summary)).toEqual(beforeSummary);
  const cleanupOnly = (await observer.cache.readCached(catalogue)).value;
  const cleanedTask = { ...beforeCatalogue.tasks[0] };
  delete cleanedTask.__task_watch_intent;
  delete cleanedTask.__task_watch_intent_cache;
  expect(cleanupOnly).toEqual({ ...beforeCatalogue, tasks: [cleanedTask] });
  expect(publications).toEqual([]);
  expect(published).toBe(false);
});

it("returns true when a legitimate recovery retry commits the response data", async () => {
  const { writer, observer, owner, contentRead, answer, detail, catalogue } = await prepareAction();
  const failure = abortFirstResponsePut(writer, detail);
  const publications = [];
  const stop = writer.cache.subscribeCache(detail, (address) => publications.push(address));
  const published = await writer.actions.cacheTaskWaitingOnResponse(scope.deviceId, scope.projectId, answer,
    { watchOwner: owner, contentRead, issuedRead: contentRead.readOrder, accept: () => true });
  stop();
  expect(failure.attempts).toHaveLength(2);
  expect(published).toBe(true);
  expect(writer.cache.cacheHealth().state).toBe("ready");
  expect(await observer.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0 });
  expect(await observer.cache.cachedAddresses({ deviceId: scope.deviceId, entityId: "", kind: "task-watch-owner" })).toEqual([]);
  const held = (await observer.cache.readCached(detail)).value;
  expect(held.task).toMatchObject({ title: answer.task.title, body: answer.task.body, watched: true });
  expect(held.timeline).toEqual(answer.timeline);
  expect((await observer.cache.readCached(catalogue)).value.tasks[0]).toMatchObject({ title: answer.task.title, watched: true });
  expect(publications).toHaveLength(1);
});
