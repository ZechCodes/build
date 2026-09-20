/** @vitest-environment jsdom */
// The issue page's route host. Thin by design — the surface is
// core/trackerIssuePage.js — so what is checked here is what the ROUTE owes it:
// the machine the project is on, its caller and its catalog, the feed, and a
// teardown that takes the surface with it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mountIssuePage = vi.fn(() => ({ feedMoved: vi.fn(), dispose: vi.fn() }));
const mountAgentRail = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));
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
  mountAgentRail.mockClear();
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

  // The rail is the shell's: the project's agent is beside an issue of the
  // project exactly as it is beside the project page, on the owner the bridge
  // answers with, so the bubble does not vanish when an issue is opened.
  it("mounts the project's agent rail beside the issue, as the project page does", async () => {
    const call = device("dev-1");
    call.mockImplementation(async (method) => (method === "project.ensure_conversation" ? { entity_id: "run-7" } : {}));
    await renderTrackerIssue();
    await flush();
    expect(call).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" });
    const [host, options] = mountAgentRail.mock.calls[0];
    expect(host.id).toBe("agent-rail");
    expect(options).toMatchObject({ kind: "project", projectId: "proj-1", entityId: "run-7", deviceId: "dev-1" });
  });

  it("takes the rail down with the page", async () => {
    await renderTrackerIssue();
    await flush();
    const rail = mountAgentRail.mock.results[0].value;
    App.viewDispose();
    App.viewDispose = null;
    expect(rail.dispose).toHaveBeenCalled();
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
