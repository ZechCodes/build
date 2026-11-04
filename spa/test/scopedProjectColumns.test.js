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
