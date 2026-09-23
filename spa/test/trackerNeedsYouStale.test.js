/** @vitest-environment jsdom */
// #119: an issue the background pass has pulled as Done must leave both
// "Needs you" surfaces, even once the pane's own filtered answer has loaded.
// Real pane, real cache, real change watcher; the wire (`callRpc`) and the
// machine's reachability are the only stand-ins.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceWatch: () => ({ away: () => false, reconnecting: () => false, moved: () => () => {} }),
}));

const DEVICE = "dev-119";
const PROJECT = "proj-1";
const reviewing = issue({ id: "issue-101", number: 101, title: "Wire 1.14.0", status: "in_review", updated_at: "2026-09-23T21:00:00Z" });
const moved = { ...reviewing, status: "done", updated_at: "2026-09-23T21:25:59Z" };

let host, pane, answer, seededAt, trackerCache, mountIssuesPane, resetChangeEvents;

const needsYouTitles = () => [...host.querySelectorAll('[data-issue-group="needsYou"] .issue-title')]
  .map((one) => one.textContent);
const dashboardNeedsYouCount = () =>
  host.querySelector('[data-dashboard-tab="needsYou"] .issue-dashboard-count')?.textContent;
const dashboardTitles = () => [...host.querySelectorAll(".issue-dashboard-title")].map((one) => one.textContent);

/** What each view says about #101 while it still needs the reader. */
const VIEWS = {
  list: {
    shows: () => expect(needsYouTitles()).toEqual(["Wire 1.14.0"]),
    cleared: () => {
      expect(needsYouTitles()).toEqual([]);
      expect(host.querySelector('[data-issue-group="needsYou"] .issue-group-count')?.textContent).toBe("0");
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
  expect(await trackerCache.issuesRecordAt(DEVICE, PROJECT)).toBeGreaterThan(seededAt);
}, WAIT);

const mount = (view) => {
  pane = mountIssuesPane(host, {
    deviceId: DEVICE,
    projectId: PROJECT,
    projectName: "Build",
    projectKey: `${DEVICE}|${PROJECT}`,
    defaultView: view,
    callRpc: (method) => (method === "issues.list" ? answer() : new Promise(() => {})),
    catalog: () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }),
  });
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountIssuesPane } = await import("../src/core/trackerIssuesPane.js"));
  ({ resetChangeEvents } = await import("../src/core/changeEvents.js"));
  await trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord([reviewing], columns()));
  seededAt = await trackerCache.issuesRecordAt(DEVICE, PROJECT);
  answer = async () => ({ issues: [reviewing] });
});

afterEach(() => {
  pane?.dispose();
  resetChangeEvents();
});

describe.each(Object.keys(VIEWS))("the %s view's Needs you", (view) => {
  it("drops an issue the next whole-list pull says is Done", async () => {
    mount(view);
    await vi.waitFor(VIEWS[view].shows, WAIT);
    await mountReadSettled(view);

    // The wire goes quiet: nothing this pane asks is answered from here on,
    // so only the cache can carry the move.
    answer = () => new Promise(() => {});
    // The background pass (core/cacheSync.js readIssues) pulls the whole list.
    await trackerCache.writeIssuesRecord(DEVICE, PROJECT, trackerCache.issuesRecord([moved], columns()));

    await vi.waitFor(VIEWS[view].cleared, WAIT);
  });
});
