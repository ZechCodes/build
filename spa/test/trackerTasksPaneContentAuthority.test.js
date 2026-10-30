// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["tasks"] }, tasks: {} }),
  watchChanges: () => ({ dispose() {} }),
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {} }));
let composerOptions;
vi.mock("../src/core/taskComposer.js", () => ({ openTaskComposer: (_host, options) => {
  composerOptions = options; return { isOpen: () => true, close() {} };
} }));

const DEVICE = "pane-content", PROJECT = "proj-1";
const stamp = "2026-10-29T12:00:00.000900Z";
const gate = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const observation = (read_order) => ({ header: { read_order, updated_at: stamp }, body: { read_order, updated_at: stamp } });
const row = (id, number, readOrder) => task({ id, project_id: PROJECT, number, title: id, status: "ready", updated_at: stamp,
  __task_content_cache: observation(readOrder) });
let tracker, reads, pane, host, rpc, list, update;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks"); composerOptions = null;
  tracker = await import("../src/core/trackerCache.js");
  reads = await import("../src/core/taskReadOrder.js");
  list = gate(); update = gate();
  rpc = vi.fn((method) => method === "tasks.list" ? list.promise : method === "tasks.update" ? update.promise : new Promise(() => {}));
});
afterEach(() => pane?.dispose());

async function mount() {
  const { mountTasksPane } = await import("../src/core/trackerTasksPane.js");
  pane = mountTasksPane(host, { deviceId: DEVICE, projectId: PROJECT, projectName: "Build", projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: "board", callRpc: rpc, catalog: () => ({ providers: [] }), feed: () => ({ items: [], workspaces: [] }) });
  await vi.waitFor(() => expect(rpc.mock.calls.filter(([method]) => method === "tasks.list")).toHaveLength(1));
}
const cachedRows = async () => (await tracker.readTasksRecord(DEVICE, PROJECT)).tasks;
const cachedRow = async (id) => (await cachedRows()).find((candidate) => candidate.id === id);
async function seed() {
  const first = await reads.nextTaskRead(), second = await reads.nextTaskRead();
  const rows = [row("moved", 2, first), row("untouched", 1, second)];
  await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord(rows, columns(), second));
  return rows;
}
async function move() {
  const card = () => host.querySelector('.task-card[data-task="moved"]');
  await vi.waitFor(() => expect(card()?.dataset.status).toBe("ready"));
  card().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
  await vi.waitFor(async () => expect((await cachedRow("moved")).status).toBe("in_progress"));
}

it("annotates actual pane whole-list wire rows with the pre-dispatch clock", async () => {
  await tracker.writeTasksRecord(DEVICE, PROJECT, tracker.tasksRecord([], columns()));
  await mount(); const boundary = await reads.nextTaskRead();
  const incoming = row("wire", 3, 1); delete incoming.__task_content_cache;
  list.resolve({ tasks: [incoming] });
  const params = rpc.mock.calls.find(([method]) => method === "tasks.list")[1];
  await vi.waitFor(async () => {
    const result = await tracker.readTasksQueryRecord(DEVICE, PROJECT, params);
    expect(result?.tasks[0]?.__task_content_cache?.header).toEqual({ read_order: result.read_order, updated_at: stamp });
    expect(result.tasks[0].__task_content_cache.body).toEqual(result.tasks[0].__task_content_cache.header);
    expect(result.read_order).toBeLessThan(boundary);
  });
});

it("retires only the moved task header provenance and preserves body and copied row observations", async () => {
  const before = await seed(); await mount(); await move();
  const moved = await cachedRow("moved"), untouched = await cachedRow("untouched");
  expect(moved.__task_content_cache?.header).toBeUndefined();
  expect(moved.__task_content_cache?.body).toEqual(before[0].__task_content_cache.body);
  expect(untouched.__task_content_cache).toEqual(before[1].__task_content_cache);
  expect(rpc.mock.calls.filter(([method]) => method === "tasks.list")).toHaveLength(1);
});

it("retires only the restored task header on move undo instead of reviving its original provenance", async () => {
  const before = await seed(); await mount(); await move();
  update.reject(new Error("move refused"));
  await vi.waitFor(async () => expect((await cachedRow("moved")).status).toBe("ready"));
  expect((await cachedRow("moved")).__task_content_cache?.header).toBeUndefined();
  expect((await cachedRow("moved")).__task_content_cache?.body).toEqual(before[0].__task_content_cache.body);
  expect((await cachedRow("untouched")).__task_content_cache).toEqual(before[1].__task_content_cache);
});

it("keeps copied rows provenance when a new task is cached without inventing observation authority at receipt", async () => {
  const before = await seed(); await mount();
  host.querySelector("[data-task-new]").click();
  composerOptions.onFiled({ task: row("new", 3, 1) }, {});
  await vi.waitFor(async () => expect((await cachedRows()).map(({ id }) => id)).toContain("new"));
  const created = await cachedRow("new");
  expect(created.__task_content_cache?.header).toBeUndefined();
  expect(created.__task_content_cache?.body).toBeUndefined();
  expect((await cachedRow("moved")).__task_content_cache).toEqual(before[0].__task_content_cache);
  expect((await cachedRow("untouched")).__task_content_cache).toEqual(before[1].__task_content_cache);
});
