import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let tracker, knowledge, edges, pages, cache;
const task = (blocked, updated_at = "2026-10-13T01:00:00Z") => ({
  id: "task", number: 7, updated_at,
  dependencies: { blocked, waiting_on: blocked ? [{ task_id: "prerequisite", resolved: false }] : [] },
});
const read = () => knowledge.readTaskDependencyKnowledge("device", "project", "task");
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  tracker = await import("../src/core/trackerCache.js");
  edges = await import("../src/core/taskWaitingOnCache.js");
  pages = await import("../src/core/trackerPages.js");
  cache = await import("../src/core/localCache.js");
  knowledge = await import("../src/core/taskDependencyKnowledge.js");
});

const writes = {
  detail: (row, options) => tracker.writeTaskRecord("device", "project", row.id, { task: row, timeline: [] }, options),
  catalogue: (row) => tracker.writeTasksRecord("device", "project", { tasks: [row] }),
  query: (row) => tracker.writeTasksQueryRecord("device", "project", { state: "open" }, { tasks: [row] }),
  page: (row) => pages.foldTasksPage(tracker.tasksAddress("device", "project"), {
    tasks: [row], above: Infinity, through: -Infinity, read: 1,
  }, () => []),
  pageRecord: (row) => pages.pullTaskPages({
    deviceId: "device", projectId: "project", params: { project_id: "project" },
    ask: async () => ({ tasks: [row] }), fold: async () => {},
  }),
  response: (row, options) => edges.cacheTaskWaitingOnResponse("device", "project", { task: row }, options),
};

it.each(Object.keys(writes))("retains %s dependency knowledge through newer unknown detail and catalogue writes", async (source) => {
  const blocked = task(true);
  await writes[source](blocked);
  for (const dependencies of [undefined, null, {}]) {
    const legacy = { ...task(false, "2026-10-13T01:02:00Z"), dependencies };
    await writes.detail(legacy);
    await writes.catalogue(legacy);
    expect(await read()).toEqual({ task_id: blocked.id, updated_at: blocked.updated_at, dependencies: blocked.dependencies });
  }
});

it.each(Object.keys(writes))("a newer capable %s empty list clears known waiting after a newer legacy write", async (source) => {
  await writes.detail(task(true));
  await writes.detail({ ...task(false, "2026-10-13T01:03:00Z"), dependencies: null });
  const ready = task(false, "2026-10-13T01:01:00Z");
  await writes[source](ready);
  expect((await read()).dependencies).toEqual(ready.dependencies);
  expect((await read()).updated_at).toBe(ready.updated_at);
});

it.each(Object.keys(writes))("an older capable %s empty list cannot clear known waiting", async (source) => {
  await writes.detail(task(true));
  await writes[source](task(false, "2026-10-13T00:59:00Z"));
  expect((await read()).dependencies.blocked).toBe(true);
});

it.each(["2026-10-13T01:00:00Z", undefined, "invalid"])("a ready version at %s cannot clear a tied or unversioned blocker", async (stamp) => {
  await writes.detail(task(true));
  await writes.catalogue({ ...task(false), updated_at: stamp });
  expect((await read()).dependencies.blocked).toBe(true);
});

it("rejected detail and action writes cannot advance dependency knowledge", async () => {
  await writes.catalogue(task(true));
  await writes.detail(task(false, "2026-10-13T01:01:00Z"), { accept: () => false });
  await writes.response(task(false, "2026-10-13T01:01:00Z"), { accept: () => false });
  expect((await read()).dependencies.blocked).toBe(true);
});

it("knowledge shares task scope and project cache eviction", async () => {
  await writes.detail(task(true));
  expect(await knowledge.readTaskDependencyKnowledge("other", "project", "task")).toBeNull();
  expect(await knowledge.readTaskDependencyKnowledge("device", "other", "task")).toBeNull();
  expect(await knowledge.readTaskDependencyKnowledge("device", "project", "other")).toBeNull();
  await cache.evictEntity("device", "project");
  expect(await read()).toBeNull();
});

it("concurrent writers and a fresh module instance retain the newest capable knowledge", async () => {
  await Promise.all([
    writes.detail(task(false)),
    writes.catalogue(task(true, "2026-10-13T01:01:00Z")),
    writes.query(task(false)),
  ]);
  await writes.detail({ ...task(false, "2026-10-13T01:02:00Z"), dependencies: null });
  vi.resetModules();
  const fresh = await import("../src/core/taskAssignmentFacts.js");
  expect((await fresh.readTaskAssignmentFacts("device", "project", "task")).dependencies.blocked).toBe(true);
});

it("consults knowledge established while an initially empty knowledge read was pending", async () => {
  const ready = task(false);
  const blocked = task(true, "2026-10-13T01:01:00Z");
  const original = cache.readCached;
  vi.spyOn(cache, "readCached").mockImplementationOnce(async (address) => {
    const absent = await original(address);
    await writes.catalogue(blocked);
    await writes.catalogue({ ...blocked, dependencies: null });
    return absent;
  });
  const { readTaskAssignmentFacts } = await import("../src/core/taskAssignmentFacts.js");
  expect((await readTaskAssignmentFacts("device", "project", "task", ready)).dependencies.blocked).toBe(true);
});

it.each([{}, { blocked: false }, { blocked: false, waiting_on: null }])("a partial ready summary %j cannot erase durable waiting", async (dependencies) => {
  await writes.detail(task(true));
  await writes.query({ ...task(false, "2026-10-13T01:01:00Z"), dependencies });
  await writes.detail({ ...task(false, "2026-10-13T01:02:00Z"), dependencies: undefined });
  expect((await read()).dependencies.blocked).toBe(true);
});
