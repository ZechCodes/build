// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const lifetime = vi.hoisted(() => ({ session: "page-watch-session" }));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => lifetime.session,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  onBridgeGreeted: () => () => {}, disarmChangeEvents() {},
  bridgeCapabilities: () => ({ tasks: {}, changes: { subscriptions: true, kinds: ["tasks"] } }),
  watchChanges: () => ({ dispose() {} }),
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
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="watch-page"></div>';
});
afterEach(() => {
  page?.dispose();
  response?.reject(new Error("Page test ended"));
  vi.restoreAllMocks();
});

async function mountPage() {
  runtime = {
    tracker: await import("../src/core/trackerCache.js"),
    content: await import("../src/core/taskContentAuthority.js"),
    watch: await import("../src/core/taskWatchAuthority.js"),
    fence: await import("../src/core/taskMutationFence.js"),
  };
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
  return { address, row, call, host, button: () => host.querySelector(".rail-watch") };
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
