// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const lifetime = vi.hoisted(() => ({ session: "page-watch-session", invalidations: [] }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => lifetime.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {}, disarmChangeEvents() {},
  bridgeCapabilities: () => ({ tasks: {}, changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: (options) => {
    if (options.onChanges) lifetime.invalidations.push(options.onChanges);
    return { dispose() {} };
  },
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: vi.fn() }));

const deviceId = "native-page-watch", projectId = "proj-1", taskId = "task-42";
const scope = { deviceId, projectId };
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
let page, runtime, response;

beforeEach(() => {
  vi.resetModules();
  lifetime.session = "page-watch-session";
  lifetime.invalidations = [];
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="watch-page"></div>';
});
afterEach(() => {
  page?.dispose();
  response?.resolve({});
  vi.restoreAllMocks();
});

async function mountPage(observeReads = false) {
  runtime = {
    tracker: await import("../src/core/trackerCache.js"),
    content: await import("../src/core/taskContentAuthority.js"),
    watch: await import("../src/core/taskWatchAuthority.js"),
    fence: await import("../src/core/taskMutationFence.js"),
  };
  const completedReads = [];
  if (observeReads) {
    const trailing = await import("../src/core/trailingRead.js"), original = trailing.trailingRead;
    vi.spyOn(trailing, "trailingRead").mockImplementation((reader, options) => original((...args) => {
      const done = Promise.resolve(reader(...args));
      if (reader.name === "readTask") completedReads.push(done);
      return done;
    }, options));
  }
  const address = runtime.tracker.taskAddress(deviceId, projectId, taskId);
  const row = task({ id: taskId, project_id: projectId, number: 42, watched: false, trackers: [], read_through: "tc-10" });
  const contentRead = await runtime.content.captureTaskContentRead(address, { revalidate: "full" });
  await runtime.tracker.writeTaskRecord(deviceId, projectId, taskId, runtime.tracker.taskRecord(row, []), { contentRead });
  const writer = vi.spyOn(runtime.tracker, "writeTaskRecord");
  response = deferred();
  const call = vi.fn((method) => method === "tasks.get" ? Promise.resolve({ task: row, timeline: [] }) : response.promise);
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  const host = document.querySelector("#watch-page");
  page = mountTaskPage(host, { ...scope, taskId, projectKey: `${deviceId}|${projectId}`, callRpc: call,
    catalog: () => ({ providers: [] }), feed: () => ({ items: [], projects: [], workspaces: [] }) });
  await vi.waitFor(() => expect(writer).toHaveBeenCalledTimes(1));
  await writer.mock.results[0].value;
  await vi.waitFor(() => expect(host.querySelector(".rail-watch")).not.toBeNull());
  return { address, row, call, host, completedReads, button: () => host.querySelector(".rail-watch") };
}

it("does not dispatch a native page watch when shared owner registration refuses", async () => {
  const mounted = await mountPage();
  const begin = vi.spyOn(runtime.watch, "beginTaskWatchMutation").mockResolvedValue(null);
  mounted.button().click();
  await vi.waitFor(() => expect(begin).toHaveBeenCalledTimes(1));
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.watch")).toHaveLength(0);
  expect(mounted.button().getAttribute("aria-pressed")).toBe("false");
});

it("registers a native page watch before RPC so an independent cache reader observes its shared pending fence", async () => {
  const databases = new Set(), original = IDBDatabase.prototype.transaction;
  vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
    databases.add(this);
    return original.apply(this, args);
  });
  const mounted = await mountPage();
  const begin = vi.spyOn(runtime.watch, "beginTaskWatchMutation");
  mounted.button().click();
  await vi.waitFor(() => expect(mounted.call).toHaveBeenCalledWith("tasks.watch", { task_id: taskId }));
  expect(begin).toHaveBeenCalledWith(scope, taskId, { watched: true, metadataOnly: true, accept: expect.any(Function) });
  expect(runtime.fence.taskMutationsPending(deviceId)).toBeGreaterThan(0);
  const firstConnections = new Set(databases);
  vi.resetModules();
  const outsideContent = await import("../src/core/taskContentAuthority.js");
  const outsideFence = await import("../src/core/taskMutationFence.js");
  expect(outsideFence.taskMutationsPending(deviceId)).toBe(0);
  const contentRead = await outsideContent.captureTaskContentRead(mounted.address, { revalidate: "full" });
  expect([...databases].some((database) => !firstConnections.has(database))).toBe(true);
  expect(contentRead.ticket.watchPending).toBe(true);
  response.resolve({ task: { ...mounted.row, watched: true, watch_started_after: "" } });
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.watch")).toHaveLength(1);
});

it("keeps fresh metadata-only begin and cleanup notifications uncertain without refilling page reads", async () => {
  const mounted = await mountPage();
  const read = vi.spyOn(runtime.watch, "readTaskWatchState");
  const before = mounted.call.mock.calls.filter(([method]) => method === "tasks.get").length;
  mounted.button().click();
  await vi.waitFor(() => expect(mounted.call).toHaveBeenCalledWith("tasks.watch", { task_id: taskId }));
  response.resolve({});
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  await vi.waitFor(() => expect(read.mock.calls.length).toBeGreaterThanOrEqual(2));
  await read.mock.results.at(-1).value;
  expect(await runtime.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(before);
});

it("cleans up a native watch owner after session replacement without publishing its obsolete acknowledgment", async () => {
  const mounted = await mountPage();
  const cache = await import("../src/core/localCache.js");
  const before = await cache.readCanonicalCached(mounted.address);
  mounted.button().click();
  await vi.waitFor(() => expect(mounted.call).toHaveBeenCalledWith("tasks.watch", { task_id: taskId }));
  lifetime.session = "replacement-session";
  response.resolve({ task: { ...mounted.row, watched: true, title: "Obsolete acknowledgment" } });
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  expect(await cache.readCanonicalCached(mounted.address)).toEqual(before);
  expect(await runtime.watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.watch")).toHaveLength(1);
});

it("does not let a stale native page acknowledgment clear another connection's newer same-task owner", async () => {
  const mounted = await mountPage();
  mounted.button().click();
  await vi.waitFor(() => expect(mounted.call).toHaveBeenCalledWith("tasks.watch", { task_id: taskId }));
  vi.resetModules();
  const outside = await import("../src/core/taskWatchAuthority.js");
  const newer = await outside.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(newer).not.toBeNull();
  expect(await outside.readTaskWatchState(scope)).toMatchObject({ pendingCount: 2 });
  response.resolve({ task: { ...mounted.row, watched: true } });
  await vi.waitFor(() => expect(runtime.fence.taskMutationsPending(deviceId)).toBe(0));
  expect(await outside.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.watched).toBe(false);
  await outside.settleTaskWatchMutation(newer, { rollback: true });
  expect(await outside.readTaskWatchState(scope)).toMatchObject({ pendingCount: 0 });
});

it("coalesces conditional eligible page control hints and unsubscribes after disposal", async () => {
  const mounted = await mountPage();
  const cache = await import("../src/core/localCache.js");
  // Conditional helper-contract fixture only; no persisted known-zero baseline.
  const read = vi.spyOn(runtime.watch, "readTaskWatchState").mockResolvedValue({ pendingCount: 0,
    watchEpoch: "eligible-epoch", pendingGeneration: "pending-generation", controlGeneration: "control-generation", uncertain: false, eligible: true });
  mounted.call.mockImplementation((method) => method === "tasks.get" ? new Promise(() => {}) : response.promise);
  const address = runtime.watch.taskWatchControlAddress(scope);
  await cache.writeCached(address, { hint: "settled" });
  await vi.waitFor(() => expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(2));
  for (let index = 0; index < 4; index += 1) await cache.writeCached(address, { hint: index });
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(2);
  page.dispose();
  const readsBefore = read.mock.calls.length;
  await cache.writeCached(address, { hint: "after-disposal" });
  expect(read).toHaveBeenCalledTimes(readsBefore);
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(2);
});

it("rejects a page eligibility result queued before disposal", async () => {
  const mounted = await mountPage();
  const cache = await import("../src/core/localCache.js");
  let resolve;
  const read = vi.spyOn(runtime.watch, "readTaskWatchState").mockImplementation(() => new Promise((yes) => { resolve = yes; }));
  await cache.writeCached(runtime.watch.taskWatchControlAddress(scope), { hint: "queued" });
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  page.dispose();
  resolve({ pendingCount: 0, watchEpoch: "eligible-epoch", controlGeneration: "control-generation", uncertain: false, eligible: true });
  await read.mock.results[0].value;
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(1);
});

async function queuedPageHint(realInvalidation) {
  const mounted = await mountPage(true);
  await mounted.completedReads[0];
  const cache = await import("../src/core/localCache.js");
  let epoch = "first-eligible-epoch";
  const read = vi.spyOn(runtime.watch, "readTaskWatchState").mockImplementation(async () => ({ pendingCount: 0,
    watchEpoch: epoch, pendingGeneration: "pending-generation", controlGeneration: "control-generation", uncertain: false, eligible: true }));
  const active = deferred();
  mounted.call.mockImplementationOnce(() => active.promise)
    .mockImplementation(async (method) => method === "tasks.get" ? { task: mounted.row, timeline: [] } : {});
  const address = runtime.watch.taskWatchControlAddress(scope);
  await cache.writeCached(address, { hint: "first" });
  await vi.waitFor(() => expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(2));
  epoch = "second-eligible-epoch";
  await cache.writeCached(address, { hint: "trailing" });
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  await read.mock.results[1].value;
  lifetime.session = "replacement-session";
  if (realInvalidation) lifetime.invalidations.at(-1)([{ tasks: { task_ids: [taskId] } }]);
  active.resolve({ task: mounted.row, timeline: [] });
  await mounted.completedReads[1];
  await Promise.all([...mounted.completedReads]);
  return mounted;
}

it("does not turn a retired page control hint into a replacement-session read", async () => {
  const mounted = await queuedPageHint(false);
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(2);
});

it("preserves a real new-session invalidation coalesced with an old page control hint", async () => {
  const mounted = await queuedPageHint(true);
  expect(mounted.call.mock.calls.filter(([method]) => method === "tasks.get")).toHaveLength(3);
});
