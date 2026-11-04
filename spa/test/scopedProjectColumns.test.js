import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, tracker, refreshScopedProjectColumns;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  ({ refreshScopedProjectColumns } = await import("../src/core/scopedProjectColumns.js"));
});
const options = (callRpc) => ({ deviceId: "d1", projectId: "p1", callRpc, active: () => true });

describe("scoped project columns metadata", () => {
  it("discards a held columns reply after the cache is cleared", async () => {
    let release;
    const callRpc = vi.fn(() => new Promise((resolve) => { release = resolve; }));
    const pending = refreshScopedProjectColumns(options(callRpc));
    await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("tasks.columns", { project_id: "p1" }));
    await cache.wipeCache();
    release({ columns: [{ id: "retired-column" }] });
    expect(await pending).toBe(false);
    expect(await tracker.readTasksRecord("d1", "p1")).toBeNull();
  });

  it("preserves existing membership and its read order when writing columns", async () => {
    const address = tracker.tasksAddress("d1", "p1");
    const task = { id: "t1", title: "Held row" };
    await cache.writeCached(address, { tasks: [task], read_order: 123, columns: [] });
    const callRpc = vi.fn(async () => ({ columns: [{ id: "custom", name: "Custom" }] }));
    expect(await refreshScopedProjectColumns(options(callRpc))).toBe(true);
    expect(await tracker.readTasksRecord("d1", "p1")).toMatchObject({ tasks: [task], read_order: 123,
      columns: [{ id: "custom", name: "Custom" }] });
  });
});

it("refreshes cached columns after rename, addition and reordering while preserving task membership", async () => {
  const address = tracker.tasksAddress("d1", "p1"), held = { id: "held-task" };
  await cache.writeCached(address, { tasks: [held], read_order: 123,
    columns: [{ id: "ready", name: "Ready" }, { id: "backlog", name: "Backlog" }] });
  const columns = [{ id: "backlog", name: "Queued" }, { id: "custom", name: "Custom" }, { id: "ready", name: "Ready" }];
  const callRpc = vi.fn(async () => ({ columns }));
  await refreshScopedProjectColumns(options(callRpc));
  expect(callRpc).toHaveBeenCalledExactlyOnceWith("tasks.columns", { project_id: "p1" });
  expect(await tracker.readTasksRecord("d1", "p1")).toMatchObject({ tasks: [held], read_order: 123, columns });
});
it("keeps newer columns when an older concurrent metadata reply lands last", async () => {
  const address = tracker.tasksAddress("d1", "p1"), task = { id: "held-task" };
  await cache.writeCached(address, { tasks: [task], read_order: 123, columns: [{ id: "ready", name: "Ready" }] });
  let releaseOlder;
  const olderRpc = vi.fn(() => new Promise((resolve) => { releaseOlder = resolve; }));
  const older = refreshScopedProjectColumns(options(olderRpc));
  await vi.waitFor(() => expect(olderRpc).toHaveBeenCalled());
  const columns = [{ id: "custom", name: "Custom" }, { id: "ready", name: "Renamed" }];
  expect(await refreshScopedProjectColumns(options(async () => ({ columns })))).toBe(true);
  const newer = await tracker.readTasksRecord("d1", "p1");
  releaseOlder({ columns: [{ id: "ready", name: "Old name" }] });
  expect(await older).toBe(false);
  expect(await tracker.readTasksRecord("d1", "p1")).toEqual(newer);
  expect(newer).toMatchObject({ tasks: [task], read_order: 123, columns });
  expect(newer.columns_read_order).toBeGreaterThan(123);
});
it("retains the column ordering clock across a legacy whole-list replacement", async () => {
  const address = tracker.tasksAddress("d1", "p1"), task = { id: "held-task" };
  let releaseOlder;
  const olderRpc = vi.fn(() => new Promise((resolve) => { releaseOlder = resolve; }));
  const older = refreshScopedProjectColumns(options(olderRpc));
  await vi.waitFor(() => expect(olderRpc).toHaveBeenCalled());
  const columns = [{ id: "custom", name: "Latest columns" }];
  expect(await refreshScopedProjectColumns(options(async () => ({ columns })))).toBe(true);
  // Legacy task-list publication replaces the list value without the helper's
  // metadata field; it must not erase independently committed column authority.
  await cache.writeCached(address, { tasks: [task], read_order: 456, columns });
  releaseOlder({ columns: [{ id: "old", name: "Obsolete columns" }] });
  expect(await older).toBe(false);
  expect(await tracker.readTasksRecord("d1", "p1")).toEqual({ tasks: [task], read_order: 456, columns });
});
it("retains the task list write stamp when a column refresh returns identical metadata", async () => {
  const address = tracker.tasksAddress("d1", "p1"), columns = [{ id: "ready", name: "Ready" }];
  await cache.writeCached(address, { tasks: [{ id: "held-task" }], read_order: 123, columns });
  const before = await cache.readCanonicalCached(address);
  expect(await refreshScopedProjectColumns(options(async () => ({ columns })))).toBe(true);
  const after = await cache.readCanonicalCached(address);
  expect(cache.recordWriteOf(after)).toBe(cache.recordWriteOf(before));
  expect(after.value).toEqual(before.value);
});
