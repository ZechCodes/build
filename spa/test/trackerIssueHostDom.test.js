/** @vitest-environment jsdom */
// The issue page's route host. Thin by design — the surface is
// core/trackerIssuePage.js — so what is checked here is what the ROUTE owes it:
// the machine the project is on, its caller and its catalog, the feed, and a
// teardown that takes the surface with it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mountIssuePage = vi.fn(() => ({ feedMoved: vi.fn(), dispose: vi.fn() }));
vi.mock("../src/core/trackerIssuePage.js", () => ({ mountIssuePage: (...args) => mountIssuePage(...args) }));

let subscribers = [];
let snapshot = { items: [], projects: [], workspaces: [], pending: [], devices: {} };
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
  refreshFeed: async () => {},
  deliverFeed: () => {},
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));

import { App } from "../src/app.js";
import { renderTrackerIssue } from "../src/views/trackerIssueView.js";
import { adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const device = (deviceId) => {
  const call = vi.fn(async () => ({}));
  adoptDeviceSession({ ...fakeSession(deviceId), call });
  return call;
};

let elsewhere;

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML =
    '<div id="toolbar"><span id="tb-verb"></span></div><div id="root"></div><aside id="agent-rail"></aside>';
  mountIssuePage.mockClear();
  subscribers = [];
  snapshot = { items: [], projects: [], workspaces: [], pending: [], devices: {} };
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
  App.devices = [
    { id: "dev-1", name: "this machine", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  App.route = { name: "trackerIssue", deviceId: "dev-1", projectId: "proj-1", issueId: "issue-1" };
  device("dev-1");
  elsewhere = device("dev-2");
});

afterEach(() => {
  expect(elsewhere).not.toHaveBeenCalled();
  App.viewDispose?.();
  App.viewDispose = null;
  resetDeviceContexts();
});

describe("the issue route", () => {
  // An issue belongs to exactly one project and never moves between projects,
  // so the project in the URL is the project — there is nothing to look up.
  it("mounts the surface on the issue and the project the URL names", async () => {
    await renderTrackerIssue();
    await flush();
    const [host, given] = mountIssuePage.mock.calls[0];
    expect(host.id).toBe("issue-pane");
    expect([given.projectId, given.deviceId, given.issueId, given.projectKey]).toEqual([
      "proj-1", "dev-1", "issue-1", "dev-1/proj-1",
    ]);
  });

  // An issue is a page OF the issues tab, so the project's tabs stand over it
  // as they do over the list: the way back to the list, and across to the
  // workspaces, from a phone that has no other way back.
  it("stands the project's tabs over the page, with Issues open", async () => {
    await renderTrackerIssue();
    await flush();
    const tabs = [...document.querySelectorAll("#project-tabs .t")];
    expect(tabs.map((tab) => tab.textContent.trim())).toEqual(["Workspaces", "Issues"]);
    expect(tabs.find((tab) => tab.classList.contains("active"))?.textContent.trim()).toBe("Issues");
  });

  it("goes to the project's list from either tab, on the same machine and project", async () => {
    await renderTrackerIssue();
    await flush();
    const [workspaces, issues] = document.querySelectorAll("#project-tabs .t");
    issues.click();
    expect(App.route).toEqual({ name: "project", deviceId: "dev-1", projectId: "proj-1", tab: "issues" });
    App.route = { name: "trackerIssue", deviceId: "dev-1", projectId: "proj-1", issueId: "issue-1" };
    await renderTrackerIssue();
    await flush();
    workspaces.isConnected || (workspaces.textContent = "");
    document.querySelector("#project-tabs .t").click();
    expect(App.route).toEqual({ name: "project", deviceId: "dev-1", projectId: "proj-1" });
  });

  it("hands over the feed the links and the assignee names are read off", async () => {
    snapshot = { ...snapshot, projects: [{ id: "proj-1" }] };
    await renderTrackerIssue();
    await flush();
    expect(mountIssuePage.mock.calls[0][1].feed().projects).toHaveLength(1);
  });

  it("repaints the surface when the feed moves, asking the bridge nothing", async () => {
    await renderTrackerIssue();
    await flush();
    const page = mountIssuePage.mock.results[0].value;
    page.feedMoved.mockClear();
    subscribers.forEach((fn) => fn(snapshot));
    expect(page.feedMoved).toHaveBeenCalled();
  });

  // A machine that cannot answer has nothing under this link to read or write.
  it("names the machine rather than standing a page up over refused calls", async () => {
    App.route = { name: "trackerIssue", deviceId: "dev-9", projectId: "proj-1", issueId: "issue-1" };
    await renderTrackerIssue();
    await flush();
    expect(mountIssuePage).not.toHaveBeenCalled();
    expect(document.querySelector("#root").textContent).not.toBe("");
  });

  it("takes the surface down with the route", async () => {
    await renderTrackerIssue();
    await flush();
    const page = mountIssuePage.mock.results[0].value;
    App.viewDispose();
    App.viewDispose = null;
    expect(page.dispose).toHaveBeenCalled();
    expect(subscribers).toHaveLength(0);
  });
});
