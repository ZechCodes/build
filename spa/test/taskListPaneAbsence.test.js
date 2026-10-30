// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

let publications, watcher;
vi.mock("../src/core/trackerCache.js", async (actual) => {
  const source = await actual();
  const tracked = (name, optionIndex) => async (...args) => {
    const result = await source[name](...args);
    if (args[optionIndex]?.accept) publications.push({ name, contentRead: args[optionIndex].contentRead });
    return result;
  };
  return { ...source, writeTasksRecord: tracked("writeTasksRecord", 3), writeTasksQueryRecord: tracked("writeTasksQueryRecord", 4) };
});
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ tasks: {} }),
  watchChanges: (registration) => { watcher = registration; return { dispose() {} }; },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
// This witness isolates the real list producer; task-detail work is unrelated
// to whether an empty list reply has authority over query/catalogue membership.
vi.mock("../src/core/trackerTaskDetailsFeed.js", () => ({
  createTrackerTaskDetailsFeed: () => ({ read: () => new Map(), updateTasks: async () => {}, dispose() {} }),
}));

const scope = { deviceId: "pane-absence", projectId: "proj-1" }, params = { project_id: "proj-1" };
const columns = [{ id: "backlog", name: "Backlog" }];
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
let pane, tracker, order;
beforeEach(async () => {
  vi.resetModules(); globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  publications = []; watcher = null;
  document.body.innerHTML = '<div id="pane-host"></div>';
  tracker = await import("../src/core/trackerCache.js");
  order = await import("../src/core/taskReadOrder.js");
});
afterEach(() => { pane?.dispose(); pane = null; });

it("passes actual pre-RPC absence proof for empty Pane replies to both replicas without deleting a blind creation", async () => {
  const original = task({ id: "original", project_id: scope.projectId });
  const initial = tracker.tasksRecord([original], columns, await order.nextTaskRead());
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, initial);
  await tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, params, initial);
  const response = deferred(), dispatched = deferred();
  let fresh = false;
  const rpc = vi.fn(async (method) => {
    if (method !== "tasks.list") return {};
    dispatched.resolve(); return fresh ? { tasks: [] } : response.promise;
  });
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(document.querySelector("#pane-host"), { ...scope, projectKey: "pane-absence|proj-1", defaultView: "dashboard",
    callRpc: rpc, feed: () => ({ items: [], projects: [], workspaces: [] }), catalog: () => ({ providers: [] }), navigate() {} });
  await dispatched.promise;
  const afterDispatch = await order.nextTaskRead();
  const local = task({ id: "local-created", project_id: scope.projectId, number: 2, title: "Local creation" });
  const localColumns = [...columns, { id: "local-column", name: "Local column" }];
  const changed = tracker.tasksRecord([local, original], localColumns, await order.nextTaskRead());
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, changed);
  await tracker.writeTasksQueryRecord(scope.deviceId, scope.projectId, params, changed);
  response.resolve({ tasks: [] });
  await vi.waitFor(() => expect(publications).toHaveLength(2));
  for (const publication of publications) expect(publication.contentRead.readOrder).toBeLessThan(afterDispatch);
  for (const record of [await tracker.readTasksRecord(scope.deviceId, scope.projectId),
    await tracker.readTasksQueryRecord(scope.deviceId, scope.projectId, params)]) {
    expect(record.tasks.map(({ id }) => id)).toEqual(["local-created", "original"]);
    expect(record.columns).toEqual(localColumns);
  }
  fresh = true; watcher.refresh();
  await vi.waitFor(() => expect(publications).toHaveLength(4));
  expect((await tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks).toEqual([]);
  expect((await tracker.readTasksQueryRecord(scope.deviceId, scope.projectId, params)).tasks).toEqual([]);
});
