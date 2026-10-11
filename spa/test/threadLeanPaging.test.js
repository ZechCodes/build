import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

it("seeks above the digest span rather than paging through omitted run rows", async () => {
  const { createThreadCache } = await import("../src/core/thread.js");
  const view = createThreadCache();
  view.seedWindow({
    items: [{ type: "event", data: { sequence: 100, event: "tool_use", summary: "Read newest.js" } }],
    deliveredSequence: 100, olderItemsRemain: true,
    activityDigests: [{ from_sequence: 2, through_sequence: 100, rows: 99, tool_calls: 99 }],
  });
  expect(view.olderPageParam()).toEqual({ before_sequence: 2 });
  // The visible floor still names the representative the pane can reach.
  expect(view.windowFloorSequence()).toBe(100);
});

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

it("widens a window above its first folded run's logical floor", async () => {
  const cache = await import("../src/core/localCache.js");
  const { widenCachedThread } = await import("../src/core/agentRailContext.js");
  const address = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "ag-1" };
  const newest = { type: "event", data: { sequence: 100, event: "tool_use", summary: "Read newest.js" } };
  await cache.writeCached(address, {
    items: [newest], deliveredSequence: 100, olderItemsRemain: true,
    activityDigests: [{ from_sequence: 2, through_sequence: 100, rows: 99 }],
  });
  const message = { type: "message", data: { sequence: 1, role: "user", body: "Start" } };
  await widenCachedThread(address, { items: [message], has_more: false }, 2);
  expect((await cache.readCached(address)).value.items).toEqual([message, newest]);
});
