// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";
import { task } from "./trackerWireFixture.js";

const App = { route: { name: "inbox" }, devices: [{ id: "list-content" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
let context, watchers;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context, liveContexts: () => [context], onDeviceStateChanged: () => () => {},
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "tasks"] }, tasks: {} }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => { watchers.push(registration); return { dispose() {} }; },
}));

const DEVICE = "list-content", PROJECT = "proj-1";
const columns = [{ id: "backlog" }, { id: "done" }];
const row = () => task({ id: "content-1", project_id: PROJECT, number: 1, title: "wire title", body: "wire body",
  updated_at: "2026-10-29T12:00:00.000900Z" });
const gate = () => { let resolve; const promise = new Promise((yes) => { resolve = yes; }); return { promise, resolve }; };
let tracker, reads, sync, script;
const rpc = vi.fn();

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  watchers = []; script = {};
  rpc.mockReset().mockImplementation(async (method, params = {}) => {
    if (script[method]) return script[method](params);
    if (method === "board.list") return { items: [] };
    if (method === "project.list") return { projects: [{ project_id: PROJECT, name: "Build" }] };
    if (method === "workspace.list") return { workspaces: [] };
    if (method === "tasks.columns") return { columns };
    if (method === "tasks.list") return { tasks: [] };
    return {};
  });
  context = { deviceId: DEVICE, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: DEVICE, active: () => true }, rpc };
  tracker = await import("../src/core/trackerCache.js");
  reads = await import("../src/core/taskReadOrder.js");
});

afterEach(async () => { if (sync) { sync.stopCacheSync(); await passesSettled(sync, DEVICE); sync = null; } });

function expectFullObservation(record, boundary) {
  const observed = record.tasks[0].__task_content_cache;
  expect(observed?.header).toEqual({ read_order: record.read_order, updated_at: row().updated_at });
  expect(observed?.body).toEqual({ read_order: record.read_order, updated_at: row().updated_at });
  expect(observed.header.read_order).toBeLessThan(boundary);
}

it("annotates actual sync phase legacy full rows with the clock captured before RPC dispatch", async () => {
  const { createTaskPhases } = await import("../src/core/syncTaskPhases.js");
  const response = gate(), dispatched = gate();
  const ask = vi.fn(async (_context, method) => {
    if (method === "tasks.columns") return { columns };
    dispatched.resolve(); return response.promise;
  });
  const pass = createTaskPhases(context, { ask, readReviews: async () => {} });
  const first = pass.first(PROJECT);
  await dispatched.promise;
  const afterDispatch = await reads.nextTaskRead();
  response.resolve({ tasks: [row()] }); await first;
  expectFullObservation(await tracker.readTasksRecord(DEVICE, PROJECT), afterDispatch);
  expect(ask.mock.calls.filter(([, method]) => method === "tasks.list")).toHaveLength(1);
});

it("annotates actual task push legacy full rows with the clock captured before RPC dispatch", async () => {
  sync = await import("../src/core/cacheSync.js");
  sync.startCacheSync(); await passesSettled(sync, DEVICE);
  const response = gate();
  script["tasks.list"] = () => response.promise;
  rpc.mockClear();
  const watcher = watchers.find(({ kinds }) => kinds.includes("thread"));
  const pending = watcher.onChanges([{ entity_id: PROJECT, tasks: { task_ids: ["content-1"] } }]);
  await called(rpc, () => rpc.mock.calls.some(([method]) => method === "tasks.list"));
  const afterDispatch = await reads.nextTaskRead();
  response.resolve({ tasks: [row()] }); await pending;
  expectFullObservation(await tracker.readTasksRecord(DEVICE, PROJECT), afterDispatch);
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.list")).toHaveLength(1);
});
