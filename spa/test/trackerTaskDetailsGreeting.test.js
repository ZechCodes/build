// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const fake = vi.hoisted(() => ({ context: null, lazy: false, waiting: false, dispatched: false }));
vi.mock("../src/core/scopedSyncRuntime.js", () => ({
  lazyViewReadsEnabled: () => fake.lazy, scopedSyncOwns: () => false, wakeScopedSync: async () => [],
}));
vi.mock("../src/core/deviceContexts.js", async (original) => ({ ...await original(),
  contextFor: () => fake.context,
  whenGreeted: async (context, dispatch) => {
    fake.waiting = true;
    await context.greeted;
    const sent = dispatch(() => true);
    fake.dispatched = true;
    return { sent, stands: () => true };
  },
}));
let release, feed;
beforeEach(async () => {
  vi.resetModules(); fake.lazy = false; fake.waiting = false; fake.dispatched = false;
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  fake.context = { active: () => true, session: {}, greeted: new Promise((resolve) => { release = resolve; }) };
});
afterEach(() => { release(); feed?.dispose(); fake.context = null; });

it("waits for current greeting before a cached summary can start legacy detail warming", async () => {
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const callRpc = vi.fn(async () => ({}));
  feed = createTrackerTaskDetailsFeed({ deviceId: "pregreet-details", projectId: "synthetic-project", callRpc });
  await feed.updateTasks([{ id: "synthetic-task", updated_at: "2026-11-01T00:00:00Z" }]);
  await vi.waitFor(() => expect(fake.waiting).toBe(true));
  expect(callRpc).not.toHaveBeenCalled();
  fake.lazy = true; release();
  await vi.waitFor(() => expect(fake.dispatched).toBe(true));
  expect(callRpc).not.toHaveBeenCalled();
});
