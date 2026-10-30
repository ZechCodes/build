// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const lifetime = vi.hoisted(() => ({ session: "control-session" }));
vi.mock("../src/core/deviceReconnect.js", () => ({ deviceSession: () => lifetime.session }));
const scope = { deviceId: "watch-control-feed", projectId: "proj-1" };
let feed;
beforeEach(() => {
  vi.resetModules();
  lifetime.session = "control-session";
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});
afterEach(() => { feed?.dispose(); vi.restoreAllMocks(); });

async function conditionalFeed() {
  const cache = await import("../src/core/localCache.js");
  const tracker = await import("../src/core/trackerCache.js");
  const watch = await import("../src/core/taskWatchAuthority.js");
  const { createTrackerTaskDetailsFeed } = await import("../src/core/trackerTaskDetailsFeed.js");
  const rows = Array.from({ length: 7 }, (_, index) => ({ id: `task-${index}`, number: index + 1,
    project_id: scope.projectId, title: "Cached", body: "Full legacy body", updated_at: "2026-10-29T00:00:00Z" }));
  for (const row of rows) await cache.writeCached(tracker.taskAddress(scope.deviceId, scope.projectId, row.id), tracker.taskRecord(row, []));
  // These are conditional admission-contract stubs, never persisted known-zero
  // authority or end-to-end fresh/reset recovery evidence.
  let state = { pendingCount: 1, watchEpoch: "pending-epoch", pendingGeneration: "pending", controlGeneration: "known-generation",
    uncertain: false, eligible: false };
  const read = vi.spyOn(watch, "readTaskWatchState").mockImplementation(async () => state);
  const call = vi.fn(() => new Promise(() => {}));
  feed = createTrackerTaskDetailsFeed({ ...scope, callRpc: call });
  await feed.updateTasks(rows);
  expect(call).not.toHaveBeenCalled();
  return { cache, watch, read, call, rows,
    setState: (next) => { state = next; },
    hint: (value = "notification", projectId = scope.projectId) => cache.writeCached(watch.taskWatchControlAddress({ ...scope, projectId }), { hint: value }) };
}

const eligible = { pendingCount: 0, watchEpoch: "settled-epoch", pendingGeneration: "settled", controlGeneration: "known-generation",
  uncertain: false, eligible: true };

it("uses conditional eligible control hints only for already admitted tasks and keeps duplicate hints within four active reads", async () => {
  const state = await conditionalFeed();
  await state.hint("foreign", "unopened-project");
  expect(state.read).not.toHaveBeenCalled();
  await state.hint("pending");
  await vi.waitFor(() => expect(state.read).toHaveBeenCalledTimes(1));
  expect(state.call).not.toHaveBeenCalled();
  state.setState(eligible);
  await state.hint("settled");
  await vi.waitFor(() => expect(state.call).toHaveBeenCalledTimes(4));
  for (let index = 0; index < 5; index += 1) await state.hint(`duplicate-${index}`);
  expect(state.call).toHaveBeenCalledTimes(4);
  expect(state.call.mock.calls.map(([method, params]) => [method, params.task_id])).toEqual(
    state.rows.slice(0, 4).map((row) => ["tasks.get", row.id]));
});

it("does not dispatch from missing or rejected control checks and can admit a later legitimate hint", async () => {
  const state = await conditionalFeed();
  state.read.mockRejectedValueOnce(new Error("Cache unavailable"));
  await state.hint("failed-check");
  await vi.waitFor(() => expect(state.read).toHaveBeenCalledTimes(1));
  expect(state.call).not.toHaveBeenCalled();
  state.setState({ pendingCount: null, watchEpoch: null, pendingGeneration: null, controlGeneration: null, uncertain: true, eligible: false });
  await state.hint("missing");
  await vi.waitFor(() => expect(state.read).toHaveBeenCalledTimes(2));
  expect(state.call).not.toHaveBeenCalled();
  state.setState(eligible);
  await state.hint("later-settlement");
  await vi.waitFor(() => expect(state.call).toHaveBeenCalledTimes(4));
});

it.each(["dispose", "session"])("refuses an eligibility answer queued before %s retirement", async (retirement) => {
  const state = await conditionalFeed();
  let resolve;
  state.read.mockImplementationOnce(() => new Promise((yes) => { resolve = yes; }));
  await state.hint("queued");
  await vi.waitFor(() => expect(state.read).toHaveBeenCalledTimes(1));
  if (retirement === "dispose") feed.dispose();
  else lifetime.session = "replacement-session";
  resolve(eligible);
  await Promise.resolve();
  expect(state.call).not.toHaveBeenCalled();
  if (retirement === "dispose") {
    await state.hint("after-dispose");
    expect(state.read).toHaveBeenCalledTimes(1);
  }
});
