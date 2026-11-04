import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task as wireTask } from "./trackerWireFixture.js";

const scope = { deviceId: "owner-hydration-device", projectId: "owner-hydration-project" };
const task = { id: "owner-hydration-task", project_id: scope.projectId, watched: true, unread_count: 3 };
const flush = async () => {
  for (let index = 0; index < 10; index += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};
let cache, createOwner, owner;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ createTaskUnreadDisplayOwner: createOwner } = await import("../src/core/taskUnreadDisplay.js"));
  owner = null;
});

afterEach(() => {
  owner?.dispose();
  vi.restoreAllMocks();
});

it("reports and notifies once when its latest internal note refresh fails", async () => {
  const onChange = vi.fn();
  owner = createOwner({ scope, consumer: "tasks-pane", onChange });
  await owner.updateTasks([task]);
  onChange.mockClear();
  const failure = new Error("Owner note refresh failed");
  const report = vi.spyOn(console, "info").mockImplementation(() => {});
  let rejectHeld;
  const held = new Promise((_, reject) => { rejectHeld = reject; });
  const read = vi.spyOn(cache, "readCachedMany").mockReturnValueOnce(held);
  const unhandled = [];
  const noteUnhandled = (error) => { unhandled.push(error); };
  process.on("unhandledRejection", noteUnhandled);
  try {
    const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
    await cache.writeCached(taskWatchIntentAddress(scope, task.id), {});
    await vi.waitFor(() => expect(read).toHaveBeenCalledOnce());
    rejectHeld(failure);
    await flush();
    expect(onChange).toHaveBeenCalledOnce();
    expect(report).toHaveBeenCalledExactlyOnceWith("Task unread display persistence unavailable", failure);
    expect(unhandled).toEqual([]);
    expect(owner.hydrating()).toBe(false);
  } finally {
    process.off("unhandledRejection", noteUnhandled);
  }
});

it("preserves direct hydration rejection while settling and notifying its caller", async () => {
  const onChange = vi.fn();
  owner = createOwner({ scope, consumer: "tasks-pane", onChange });
  const failure = new Error("Direct owner hydration failed");
  const report = vi.spyOn(console, "info").mockImplementation(() => {});
  vi.spyOn(cache, "readCachedMany").mockRejectedValueOnce(failure);
  await expect(owner.updateTasks([task])).rejects.toBe(failure);
  expect(onChange).toHaveBeenCalledOnce();
  expect(report).toHaveBeenCalledExactlyOnceWith("Task unread display persistence unavailable", failure);
  expect(owner.hydrating()).toBe(false);
});

it("starts idle without reading cache until tasks are supplied", () => {
  const read = vi.spyOn(cache, "readCachedMany");
  owner = createOwner({ scope, consumer: "tasks-pane" });
  expect(read).not.toHaveBeenCalled();
  expect(owner.hydrating()).toBe(false);
});

it("accepts a cloned current task snapshot after hydration", async () => {
  owner = createOwner({ scope, consumer: "tasks-pane" });
  await owner.updateTasks([task]);
  owner.rendered(structuredClone(task), null, 3);
  const incomplete = { ...task };
  delete incomplete.unread_count;
  expect(owner.preview(incomplete, null)).toBe(3);
});

it("does not replace a retained display with a premature render while its note is hydrating", async () => {
  const tracker = await import("../src/core/trackerCache.js");
  const { captureTaskContentRead } = await import("../src/core/taskContentAuthority.js");
  const address = tracker.tasksAddress(scope.deviceId, scope.projectId);
  const contentRead = await captureTaskContentRead(address, { revalidate: "full" });
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    tracker.tasksRecord([wireTask({ ...task, unread_count: 3 })], [], contentRead.readOrder), { contentRead });
  const selected = (await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0];
  owner = createOwner({ scope, consumer: "tasks-pane" });
  await owner.updateTasks([selected]);
  owner.rendered(selected, null, 3, address);
  const { taskWatchIntentAddress } = await import("../src/core/taskReadOrder.js");
  const noteAddress = taskWatchIntentAddress(scope, task.id);
  await vi.waitFor(async () => expect((await cache.readCached(noteAddress))?.value
    ?.task_unread_display?.["tasks-pane"]?.total).toBe(3));
  owner.dispose();
  const incomplete = { ...selected };
  for (const key of ["unread_count", "unread_mentions", "unread_agent_comments", "__task_family_cache"]) delete incomplete[key];
  owner = createOwner({ scope, consumer: "tasks-pane" });
  const read = cache.readCachedMany;
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  let waiting = false;
  vi.spyOn(cache, "readCachedMany").mockImplementation(async (addresses) => {
    const records = await read(addresses);
    if (addresses.length === 4 && addresses[0].kind === "task-content-generation") {
      waiting = true;
      await held;
    }
    return records;
  });
  const hydration = owner.updateTasks([incomplete]);
  try {
    await vi.waitFor(() => expect(waiting).toBe(true));
    owner.rendered(incomplete, { task: incomplete, timeline: [] }, 0, address);
  } finally {
    release();
  }
  await hydration;
  expect(owner.preview(incomplete, null)).toBe(3);
  expect((await cache.readCached(noteAddress)).value.task_unread_display["tasks-pane"].total).toBe(3);
});
