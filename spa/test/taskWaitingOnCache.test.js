import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, tracker, edges, pages, order;
const device = "d", project = "p";
const task = { id: "t", number: 7, project_id: project, title: "Work", state: "open", status: "ready", updated_at: "2026-10-10T00:00:00Z" };
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  edges = await import("../src/core/taskWaitingOnCache.js");
  pages = await import("../src/core/trackerPages.js");
  order = await import("../src/core/taskReadOrder.js");
});

it("writes mutation replies to detail and every cached list without losing timeline", async () => {
  const timeline = [{ id: "event", kind: "created" }];
  await tracker.writeTaskRecord(device, project, task.id, tracker.taskRecord(task, timeline));
  const addresses = [tracker.tasksAddress(device, project), tracker.tasksQueryAddress(device, project, { state: "open" }), tracker.tasksPageAddress(device, project, { limit: 100 })];
  await Promise.all(addresses.map((address) => cache.writeCached(address, { tasks: [task, { id: "other" }], columns: [{ id: "ready" }], next_cursor: "next" })));
  const next = { ...task, dependencies: { blocked: true, waiting_on: [{ task_id: "blocker" }] } };
  await edges.cacheTaskWaitingOnResponse(device, project, { task: next });
  expect((await tracker.readTaskRecord(device, project, task.id)).timeline).toEqual(timeline);
  for (const address of addresses) {
    const held = (await cache.readCached(address)).value;
    expect(held.tasks[0].dependencies).toEqual(next.dependencies);
    expect(held.tasks[1]).toEqual({ id: "other" });
    expect(held.next_cursor).toBe("next");
  }
});

it("a page asked before the mutation cannot undo its dependency facts", async () => {
  const address = tracker.tasksAddress(device, project);
  await cache.writeCached(address, tracker.tasksRecord([task], []));
  const read = await order.nextTaskRead();
  const next = { ...task, dependencies: { blocked: true, waiting_on: [{ task_id: "blocker" }] } };
  await edges.cacheTaskWaitingOnResponse(device, project, { task: next });
  await pages.foldTasksPage(address, { tasks: [task], read, above: Infinity, through: -Infinity }, () => []);
  expect((await tracker.readTasksRecord(device, project)).tasks[0].dependencies).toEqual(next.dependencies);
});

it("fences superseded answers and retains a newer task and timeline", async () => {
  const newer = { ...task, title: "Newer", updated_at: "2026-10-11T00:00:00Z" };
  await tracker.writeTaskRecord(device, project, task.id, tracker.taskRecord(newer, [{ id: "new" }]));
  await edges.cacheTaskWaitingOnResponse(device, project, { task });
  expect((await tracker.readTaskRecord(device, project, task.id)).task.title).toBe("Newer");
  await edges.cacheTaskWaitingOnResponse(device, project, { task: { ...newer, title: "Superseded" } }, { accept: () => false });
  expect((await tracker.readTaskRecord(device, project, task.id)).task.title).toBe("Newer");
});

it("reverse pages append by id and a fresh first page replaces the cached reverse list", async () => {
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [{ task_id: "a" }], next_cursor: "one" });
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [{ task_id: "a", title: "updated" }, { task_id: "b" }] }, { cursor: "one" });
  expect(await edges.readTaskWaitingOnBlocks(device, project, "t")).toEqual({ blocks: [{ task_id: "a", title: "updated" }, { task_id: "b" }], next_cursor: null });
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [] });
  expect((await edges.readTaskWaitingOnBlocks(device, project, "t")).blocks).toEqual([]);
});

it("files a task into the catalog and matching queries, retaining filtered membership", async () => {
  const matches = tracker.tasksQueryAddress(device, project, { state: "open", assignee: { kind: "user" } });
  const misses = tracker.tasksQueryAddress(device, project, { state: "open", assignee: "none" });
  await cache.writeCached(matches, tracker.tasksRecord([], []));
  await cache.writeCached(misses, tracker.tasksRecord([], []));
  const filed = { ...task, assignee: { kind: "user" } };
  await edges.cacheTaskWaitingOnResponse(device, project, { task: filed });
  expect((await tracker.readTasksRecord(device, project)).tasks.map((row) => row.id)).toEqual([task.id]);
  expect((await cache.readCached(matches)).value.tasks.map((row) => row.id)).toEqual([task.id]);
  expect((await cache.readCached(misses)).value.tasks).toEqual([]);
  await edges.cacheTaskWaitingOnResponse(device, project, { task: { ...task, assignee: null } });
  expect((await cache.readCached(matches)).value.tasks).toEqual([]);
  expect((await cache.readCached(misses)).value.tasks.map((row) => row.id)).toEqual([task.id]);
});

it("a reverse continuation starts from the authoritative detail preview", async () => {
  await tracker.writeTaskRecord(device, project, "t", tracker.taskRecord({ ...task, dependencies: {
    blocks: [{ task_id: "preview" }], blocks_next_cursor: "preview-cursor", blocks_count: 2,
  } }));
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [{ task_id: "later" }] }, { cursor: "preview-cursor" });
  expect((await edges.readTaskWaitingOnBlocks(device, project, "t")).blocks.map((row) => row.task_id)).toEqual(["preview", "later"]);
});

it("new detail dependency facts retire the old reverse chain, and stale first pages cannot replace a newer read", async () => {
  const older = await edges.captureTaskWaitingOnBlocksRead(device, project, "t");
  const newerRead = await edges.captureTaskWaitingOnBlocksRead(device, project, "t");
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [{ task_id: "fresh" }] }, newerRead);
  await edges.cacheTaskWaitingOnBlocks(device, project, "t", { blocks: [{ task_id: "stale" }] }, older);
  expect((await edges.readTaskWaitingOnBlocks(device, project, "t")).blocks.map((row) => row.task_id)).toEqual(["fresh"]);
  await tracker.writeTaskRecord(device, project, "t", tracker.taskRecord({ ...task, dependencies: { blocks: [], blocks_count: 0 } }));
  expect(await edges.readTaskWaitingOnBlocks(device, project, "t")).toBeNull();
});

it("forward edge removal and addition retire the reverse endpoint even outside its preview", async () => {
  const blocker = { ...task, id: "blocker", dependencies: { blocks: [{ task_id: "preview" }], blocks_count: 3, blocks_next_cursor: "next" } };
  await tracker.writeTaskRecord(device, project, blocker.id, tracker.taskRecord(blocker));
  await tracker.writeTaskRecord(device, project, task.id, tracker.taskRecord({ ...task, dependencies: { waiting_on: [{ task_id: blocker.id }] } }));
  await edges.cacheTaskWaitingOnBlocks(device, project, blocker.id, { blocks: [{ task_id: "preview" }, { task_id: task.id }], next_cursor: "later" });
  const beforeRemoval = await edges.captureTaskWaitingOnBlocksRead(device, project, blocker.id);
  await edges.cacheTaskWaitingOnResponse(device, project, { task: { ...task, dependencies: { waiting_on: [] } } });
  expect(await edges.readTaskWaitingOnBlocks(device, project, blocker.id)).toBeNull();
  await edges.cacheTaskWaitingOnBlocks(device, project, blocker.id, { blocks: [{ task_id: task.id }] }, beforeRemoval);
  expect(await edges.readTaskWaitingOnBlocks(device, project, blocker.id)).toBeNull();
  const afterRemoval = await edges.captureTaskWaitingOnBlocksRead(device, project, blocker.id);
  await edges.cacheTaskWaitingOnBlocks(device, project, blocker.id, { blocks: [{ task_id: "preview" }] }, afterRemoval);
  const beforeAddition = await edges.captureTaskWaitingOnBlocksRead(device, project, blocker.id);
  await edges.cacheTaskWaitingOnResponse(device, project, { task: { ...task, dependencies: { waiting_on: [{ task_id: blocker.id }] } } });
  expect(await edges.readTaskWaitingOnBlocks(device, project, blocker.id)).toBeNull();
  await edges.cacheTaskWaitingOnBlocks(device, project, blocker.id, { blocks: [{ task_id: "preview" }] }, beforeAddition);
  expect(await edges.readTaskWaitingOnBlocks(device, project, blocker.id)).toBeNull();
});

it("a project push retires off-preview paging and fences unseen in-flight first pages", async () => {
  await tracker.writeTaskRecord(device, project, "blocker", tracker.taskRecord({ ...task, id: "blocker", dependencies: {
    blocks: [{ task_id: "preview" }], blocks_next_cursor: "next", blocks_count: 3,
  } }));
  await edges.cacheTaskWaitingOnBlocks(device, project, "blocker", { blocks: [{ task_id: "off-preview", title: "Old", state: "open" }] }, { cursor: "next" });
  const oldPage = await edges.captureTaskWaitingOnBlocksRead(device, project, "blocker");
  const unseenFirstPage = await edges.captureTaskWaitingOnBlocksRead(device, project, "unseen-blocker");
  await edges.invalidateTaskWaitingOnBlocks(device, project);
  expect(await edges.readTaskWaitingOnBlocks(device, project, "blocker")).toBeNull();
  await edges.cacheTaskWaitingOnBlocks(device, project, "blocker", { blocks: [{ task_id: "off-preview", title: "Old", state: "open" }] }, oldPage);
  await edges.cacheTaskWaitingOnBlocks(device, project, "unseen-blocker", { blocks: [{ task_id: "off-preview", title: "Old", state: "open" }] }, unseenFirstPage);
  expect(await edges.readTaskWaitingOnBlocks(device, project, "blocker")).toBeNull();
  expect(await edges.readTaskWaitingOnBlocks(device, project, "unseen-blocker")).toBeNull();
  const newPage = await edges.captureTaskWaitingOnBlocksRead(device, project, "blocker");
  await edges.cacheTaskWaitingOnBlocks(device, project, "blocker", { blocks: [{ task_id: "off-preview", title: "New", state: "closed" }] }, { ...newPage, cursor: "next" });
  expect((await edges.readTaskWaitingOnBlocks(device, project, "blocker")).blocks).toEqual([
    { task_id: "preview" }, { task_id: "off-preview", title: "New", state: "closed" },
  ]);
});
