import { readFileSync } from "node:fs";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, tracker, index, feedModule, counts, reads;
const place = { deviceId: "device-ref", projectId: "project-ref" };
const projectKey = "device-ref/project-ref";
const task = { id: "task-42", number: 42, title: "Known positive" };
const feed = { projects: [{ ...place, id: place.projectId, projectKey }], workspaces: [], items: [] };
let stop;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  index = await import("../src/core/referenceIndex.js");
  feedModule = await import("../src/core/referenceIndexFeed.js");
  counts = await import("../src/core/taskAggregateReads.js");
  reads = await import("../src/core/taskReadOrder.js");
});
afterEach(() => { stop?.(); stop = null; });
const resolve = (number) => index.referenceResolver({ place }).task(number);

it("does not infer absence from partial membership while preserving legacy full catalog misses", () => {
  index.holdReferenceSources({ feed, tasks: { [projectKey]: [task] }, tasksComplete: { [projectKey]: false } });
  expect(resolve(42)).toMatchObject({ taskId: task.id });
  expect(resolve(43)).toBeUndefined();
  index.holdReferenceSources({ feed, tasks: { [projectKey]: [task] }, tasksComplete: { [projectKey]: true } });
  expect(resolve(43)).toBeNull();
});

it("distinguishes point unknown, found and explicit missing independently of partial lists", () => {
  const base = { feed, tasks: { [projectKey]: [] }, tasksComplete: { [projectKey]: false } };
  index.holdReferenceSources({ ...base, taskLookups: { [projectKey]: { 42: { status: "unknown" } } } });
  expect(resolve(42)).toBeUndefined();
  const before = index.referenceIndexVersion();
  index.holdReferenceSources({ ...base, taskLookups: { [projectKey]: { 42: { status: "found", task } } } });
  expect(index.referenceIndexVersion()).toBeGreaterThan(before);
  expect(resolve(42)).toMatchObject({ taskId: task.id, title: task.title });
  index.holdReferenceSources({ ...base, taskLookups: { [projectKey]: { 42: { status: "missing", task: null } } } });
  expect(resolve(42)).toBeNull();
});

it("marks folded legacy pages partial until their read stretches cover every task number", async () => {
  const address = tracker.tasksAddress(place.deviceId, place.projectId);
  await cache.writeCached(address, { tasks: [task] });
  await cache.writeCached(reads.readsAddress(address), { stretches: [{ above: Infinity, through: 42, read: 1 }], rows: {} });
  stop = feedModule.feedReferenceIndex({ subscribeFeed(listener) { listener(feed); return () => {}; } });
  await vi.waitFor(() => expect(resolve(42)?.taskId).toBe(task.id));
  expect(resolve(1)).toBeUndefined();
  await cache.writeCached(reads.readsAddress(address), { stretches: [
    { above: Infinity, through: 42, read: 1 }, { above: 42, through: -Infinity, read: 2 },
  ], rows: {} });
  await vi.waitFor(() => expect(resolve(1)).toBeNull());
});

it("reads exact lookup cache only after explicit demand and follows subsequent point answers", async () => {
  await cache.writeCached(counts.taskLookupAddress({ deviceId: place.deviceId, projectId: place.projectId }, 42), {
    generation: 1, result: { project_id: place.projectId, number: 42, task, identities: {}, basis: "basis", status: "found" },
  });
  stop = feedModule.feedReferenceIndex({ subscribeFeed(listener) { listener(feed); return () => {}; } });
  expect(resolve(42)).toBeUndefined();
  const demand = await import("../src/core/taskReferenceDemand.js");
  const release = demand.admitTaskReferenceLookups(place, [42]);
  try {
    await vi.waitFor(() => expect(resolve(42)?.taskId).toBe(task.id));
    await cache.writeCached(counts.taskLookupAddress(place, 42), { generation: 2,
      result: { project_id: place.projectId, number: 42, task: null, identities: {}, basis: "next", status: "missing" } });
    await vi.waitFor(() => expect(resolve(42)).toBeNull());
  } finally { release(); }
});

it("does not treat an action-created one-row legacy catalogue as full-list absence proof", async () => {
  const actions = await import("../src/core/taskWaitingOnCache.js");
  await actions.cacheTaskWaitingOnResponse(place.deviceId, place.projectId, {
    task: { ...task, project_id: place.projectId, state: "open", status: "backlog" },
  });
  stop = feedModule.feedReferenceIndex({ subscribeFeed(listener) { listener(feed); return () => {}; } });
  await vi.waitFor(() => expect(resolve(42)?.taskId).toBe(task.id));
  expect(resolve(1)).toBeUndefined();
});

it("keeps a cacheNewTask-shaped one-row catalogue without read notes unknown for other numbers", async () => {
  const pane = readFileSync(new URL("../src/core/trackerTasksPane.js", import.meta.url), "utf8");
  expect(pane).toContain("async function cacheNewTask(task)");
  expect(pane).toContain("writeTasksRecord(state.deviceId, state.projectId, tasksRecord(catalogue, state.columns, filedAs))");
  await tracker.writeTasksRecord(place.deviceId, place.projectId, tracker.tasksRecord([task], [], 1));
  expect(await cache.readCached(reads.readsAddress(tracker.tasksAddress(place.deviceId, place.projectId)))).toBeUndefined();
  stop = feedModule.feedReferenceIndex({ subscribeFeed(listener) { listener(feed); return () => {}; } });
  await vi.waitFor(() => expect(resolve(42)?.taskId).toBe(task.id));
  expect(resolve(1)).toBeUndefined();
});

it("preserves explicit complete legacy catalogue absence semantics", async () => {
  await cache.writeCached(tracker.tasksAddress(place.deviceId, place.projectId), { tasks: [task], catalog_complete: true });
  stop = feedModule.feedReferenceIndex({ subscribeFeed(listener) { listener(feed); return () => {}; } });
  await vi.waitFor(() => expect(resolve(42)?.taskId).toBe(task.id));
  expect(resolve(1)).toBeNull();
});
