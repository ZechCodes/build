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
