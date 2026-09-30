/** @vitest-environment jsdom */
// #273: the Workspaces tab asks its machine for every workspace's size once
// when it opens, and paints from the cache as the sizes arrive. A row with no
// size yet shows a quiet placeholder on a machine that will send one; the
// greeting that says so is the real one (fixtures/api/v1/session.hello.json),
// through the real greeting path into the real cache.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: () => ({ dispose: vi.fn() }) }));
vi.mock("../src/core/createWork.js", () => ({ openCreateWork: vi.fn() }));
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: vi.fn() }));
vi.mock("../src/core/trackerTasksPane.js", () => ({ mountTasksPane: () => ({ feedMoved: vi.fn(), dispose: vi.fn() }) }));

let subscribers = [];
let snapshot;
const deliver = () => subscribers.forEach((fn) => fn(snapshot));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscribers.push(fn);
    fn(snapshot);
    return () => {
      subscribers = subscribers.filter((each) => each !== fn);
    };
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
  deliverFeed: () => deliver(),
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));

const { App } = await import("../src/app.js");
const { renderProject } = await import("../src/views/projectView.js");
const { pressProjectTab } = await import("../src/core/toolbar.js");
const { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { standShell, stopShell } = await import("../src/core/shell.js");
const { fakeSession } = await import("./deviceSessionFixture.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const olderGreeting = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "workspace.measure_sizes") };

const flush = () => new Promise((settle) => setTimeout(settle, 0));

const workspace = (id, extra = {}) => ({
  id,
  workspace_id: id,
  project_id: "proj-1",
  deviceId: "dev-1",
  projectKey: "dev-1/proj-1",
  workspaceKey: `dev-1/${id}`,
  status: "ready",
  name: id,
  updated_at: "2026-01-01T00:00:00Z",
  directories: [{ source_id: "repo", branch: "build/login", is_git: true }],
  ...extra,
});
const project = { id: "proj-1", project_id: "proj-1", name: "Build", deviceId: "dev-1", projectKey: "dev-1/proj-1" };
const sized = (bytes) => ({
  measured_at_ms: 0, last_activity_ms: null, idle: false, reclaimable: false, holds: [], tasks: [],
  dirty_files: 0, unpushed_commits: 0, behind_commits: 0, size_bytes: bytes, size_measured_at_ms: 1,
  pruned_bytes: 0, pruned_at_ms: null, noticed_at_ms: null,
});

let call;

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  localStorage.clear();
  document.body.innerHTML =
    '<div id="toolbar"><span id="tb-verb"></span></div><div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  subscribers = [];
  snapshot = { items: [], projects: [project], workspaces: [workspace("ws-1"), workspace("ws-2")], pending: [], devices: {} };
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
  App.devices = [{ id: "dev-1", name: "this machine", status: "online" }];
  App.route = { name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "workspaces" };
  location.hash = "#/device/dev-1/project/proj-1/workspaces";
  call = vi.fn(async (method) =>
    method === "project.ensure_conversation" ? { project_id: "proj-1", entity_id: "run-7", run_id: "run-7" } : { queued: [] },
  );
  adoptBridgeSelection(adoptDeviceSession({ ...fakeSession("dev-1"), call }), { version: "3.4.0" }, null);
});

afterEach(() => {
  App.viewDispose?.();
  App.viewDispose = null;
  stopShell();
  resetDeviceContexts();
  resetChangeEvents();
});

const greet = (hello) => greetBridge(async (method) => (method === "session.hello" ? hello : {}), { deviceId: "dev-1", strict: true });
const open = async () => {
  standShell(App.route);
  await renderProject();
  await flush();
};
const sizeAsks = () => call.mock.calls.filter(([method]) => method === "workspace.measure_sizes");
const sizes = () => [...document.querySelectorAll("[data-workspace] .project-size")].map((cell) => cell.textContent);

it("asks the machine once for the project's sizes when the tab opens", async () => {
  await greet(greeting);
  await open();

  deliver();
  await flush();

  expect(sizeAsks()).toEqual([["workspace.measure_sizes", { project_id: "proj-1" }]]);
});

it("does not ask while the Tasks tab is open, and asks on switching to Workspaces", async () => {
  await greet(greeting);
  App.route = { ...App.route, tab: "tasks" };
  await open();
  expect(sizeAsks()).toEqual([]);

  pressProjectTab("workspaces");
  await flush();

  expect(sizeAsks()).toHaveLength(1);
});

it("shows a quiet placeholder for a size still to come, and the size when it lands", async () => {
  await greet(greeting);
  await open();
  await vi.waitFor(() => expect(sizes()).toEqual(["—", "—"]));
  expect(document.querySelector("[data-workspace] .project-size").classList.contains("project-size-pending")).toBe(true);

  snapshot = { ...snapshot, workspaces: [workspace("ws-1", { lifecycle: sized(3_000_000_000) }), workspace("ws-2")] };
  deliver();

  expect(sizes()).toEqual(["3.0 GB", "—"]);
  expect(document.querySelector("[data-workspace] .project-size").classList.contains("project-size-pending")).toBe(false);
});

it("paints the placeholder from the cache before the machine answers again", async () => {
  await greet(greeting);
  await flush();
  resetChangeEvents(); // the session is gone; the cache still says what the machine does

  await open();

  await vi.waitFor(() => expect(sizes()).toEqual(["—", "—"]));
});

it("shows nothing for a missing size, and asks nothing, on an older machine", async () => {
  await greet(olderGreeting);
  await open();
  await flush();

  expect(sizes()).toEqual(["", ""]);
  expect(sizeAsks()).toEqual([]);
});
