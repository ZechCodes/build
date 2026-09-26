/** @vitest-environment jsdom */
// Real pane, dashboard projection, cache, and change watcher wiring. Only the
// remote RPC is held open so every visible change has to come from cache.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountIssuesPane } from "../src/core/trackerIssuesPane.js";
import { resetChangeEvents } from "../src/core/changeEvents.js";
import { issuesRecord, writeIssuesRecord } from "../src/core/trackerCache.js";
import { columns, issue } from "./trackerWireFixture.js";

let pane;
let host;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
});

afterEach(() => {
  pane?.dispose();
  resetChangeEvents();
});

it("wires a mounted issues pane to cached dashboard tabs and live counts", async () => {
  const review = issue({ id: "review", number: 7, title: "Review changes", status: "in_review" });
  await writeIssuesRecord("dashboard-device", "dashboard-project", issuesRecord([review], columns()));
  pane = mountIssuesPane(host, {
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
  await vi.waitFor(() => expect(host.querySelector(".issue-dashboard-title")?.textContent).toBe("Review changes"));
  expect(host.querySelector('[data-dashboard-tab="needsYou"] .issue-dashboard-count').textContent).toBe("1");

  host.querySelector('[data-dashboard-tab="inProgress"]').click();
  await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("inProgress"));
  expect(host.querySelector(".issue-dashboard-empty")?.textContent).toBe("No agent holds an issue.");

  const assigned = issue({ id: "assigned", number: 8, title: "Take a look", assignee: { kind: "user" } });
  await writeIssuesRecord("dashboard-device", "dashboard-project", issuesRecord([assigned, review], columns()));
  await vi.waitFor(() => expect(host.querySelector('[data-dashboard-tab="needsYou"] .issue-dashboard-count')?.textContent).toBe("2"));
  host.querySelector('[data-dashboard-tab="needsYou"]').click();
  await vi.waitFor(() => expect([...host.querySelectorAll(".issue-dashboard-title")].map((one) => one.textContent))
    .toEqual(["Take a look", "Review changes"]));
});
