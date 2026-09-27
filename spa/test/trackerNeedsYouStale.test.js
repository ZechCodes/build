/** @vitest-environment jsdom */
// #119: a task the background pass has pulled as Done must leave both
// "Needs you" surfaces, even once the pane's own filtered answer has loaded.
// Real pane, real cache, real change watcher; the wire (`callRpc`) and the
// machine's reachability are the only stand-ins.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-119";
const PROJECT = "proj-1";
const reviewing = task({ id: "task-101", number: 101, title: "Wire 1.14.0", status: "in_review", updated_at: "2026-09-23T21:00:00Z" });
const moved = { ...reviewing, status: "done", updated_at: "2026-09-23T21:25:59Z" };

let host, pane, answer, seededAt, trackerCache, mountTasksPane, resetChangeEvents;

const needsYouTitles = () => [...host.querySelectorAll('[data-task-group="needsYou"] .task-title')]
  .map((one) => one.textContent);
const dashboardNeedsYouCount = () =>
  host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count')?.textContent;
const dashboardTitles = () => [...host.querySelectorAll(".task-dashboard-title")].map((one) => one.textContent);

/** What each view says about #101 while it still needs the reader. */
const VIEWS = {
  list: {
    shows: () => expect(needsYouTitles()).toEqual(["Wire 1.14.0"]),
    cleared: () => {
      expect(needsYouTitles()).toEqual([]);
      expect(host.querySelector('[data-task-group="needsYou"] .task-group-count')?.textContent).toBe("0");
    },
  },
  dashboard: {
    shows: () => {
      expect(dashboardNeedsYouCount()).toBe("1");
      expect(dashboardTitles()).toEqual(["Wire 1.14.0"]);
    },
    cleared: () => {
      expect(dashboardNeedsYouCount()).toBe("0");
      expect(dashboardTitles()).toEqual([]);
    },
  },
};

const WAIT = { timeout: 5000 };

/** The mount's own read is done writing. An unnarrowed answer (the
 *  Dashboard's) is also written as the whole list, and a pull that lands
 *  before that write would be overwritten by the older answer. */
const mountReadSettled = (view) => vi.waitFor(async () => {
  if (view === "list") return;
  expect(await trackerCache.tasksRecordAt(DEVICE, PROJECT)).toBeGreaterThan(seededAt);
}, WAIT);

const mount = (view) => {
  pane = mountTasksPane(host, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectName: "Build",
    projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: view,
    callRpc: (method) => (method === "tasks.list" ? answer() : new Promise(() => {})),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="tasks"></div>';
  host = document.querySelector("#tasks");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountTasksPane } = await import("../src/core/trackerTasksPane.js"));
  ({ resetChangeEvents } = await import("../src/core/changeEvents.js"));
  await trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord([reviewing], columns()));
  seededAt = await trackerCache.tasksRecordAt(DEVICE, PROJECT);
  answer = async () => ({ tasks: [reviewing] });
});

afterEach(() => {
  pane?.dispose();
  resetChangeEvents();
});

describe.each(Object.keys(VIEWS))("the %s view's Needs you", (view) => {
  it("drops a task the next whole-list pull says is Done", async () => {
    mount(view);
    await vi.waitFor(VIEWS[view].shows, WAIT);
    await mountReadSettled(view);

    // The wire goes quiet: nothing this pane asks is answered from here on,
    // so only the cache can carry the move.
    answer = () => new Promise(() => {});
    // The background pass (core/cacheSync.js readTasks) pulls the whole list.
    await trackerCache.writeTasksRecord(DEVICE, PROJECT, trackerCache.tasksRecord([moved], columns()));

    await vi.waitFor(VIEWS[view].cleared, WAIT);
  });
});
