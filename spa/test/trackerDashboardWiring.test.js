/** @vitest-environment jsdom */
// Real pane, dashboard projection, cache, and change watcher wiring. Only the
// remote RPC is held open so every visible change has to come from cache.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountTasksPane } from "../src/core/trackerTasksPane.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { tasksRecord, writeTasksRecord } from "../src/core/trackerCache.js";
import { columns, task } from "./trackerWireFixture.js";

let pane;
let host;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
});

afterEach(() => {
  pane?.dispose();
  resetChangeEvents();
});

it("wires a mounted tasks pane to cached dashboard tabs and live counts", async () => {
  const review = task({ id: "review", number: 7, title: "Review changes", status: "in_review" });
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([review], columns()));
  pane = mountTasksPane(host, {
    deviceId: "dashboard-device",
    projectId: "dashboard-project",
    projectName: "Build",
    projectKey: "dashboard-project",
    defaultView: "dashboard",
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });

  await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("needsYou"));
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-title")?.textContent).toBe("Review changes"));
  expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count').textContent).toBe("1");

  host.querySelector('[data-dashboard-tab="active"]').click();
  await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("active"));
  expect(host.querySelector(".task-dashboard-empty")?.textContent).toBe("No one holds a task right now.");

  const assigned = task({ id: "assigned", number: 8, title: "Take a look", assignee: { kind: "user" } });
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([assigned, review], columns()));
  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count')?.textContent).toBe("2"));
  host.querySelector('[data-dashboard-tab="needsYou"]').click();
  await vi.waitFor(() => expect([...host.querySelectorAll(".task-dashboard-title")].map((one) => one.textContent))
    .toEqual(["Take a look", "Review changes"]));
});

it("moves a mounted task between Active groups when the cached feed changes", async () => {
  const held = task({ id: "task", number: 8, title: "Review the draft", assignee: { kind: "agent", agent_id: "agent-1" } });
  let feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: false }] }] };
  await writeTasksRecord("dashboard-device", "dashboard-project", tasksRecord([held], columns()));
  pane = mountTasksPane(host, {
    deviceId: "dashboard-device",
    projectId: "dashboard-project",
    projectName: "Build",
    projectKey: "dashboard-project",
    defaultView: "dashboard",
    callRpc: () => new Promise(() => {}),
    catalog: () => ({ providers: [] }),
    feed: () => feed,
  });

  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-tab="active"]')).not.toBeNull());
  host.querySelector('[data-dashboard-tab="active"]').click();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Assigned"));
  expect(host.querySelector(".task-dashboard-detail")?.textContent).toContain("Backlog");

  feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: true }] }] };
  pane.feedMoved();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Working"));
  expect(host.querySelector(".task-dashboard-detail")?.textContent).toContain("working now");

  feed = { items: [{ projectKey: "dashboard-project", agents: [{ id: "agent-1", working: false }] }] };
  pane.feedMoved();
  await vi.waitFor(() => expect(host.querySelector(".task-dashboard-group-title")?.textContent).toBe("Assigned"));
});
