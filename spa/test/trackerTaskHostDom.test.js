/** @vitest-environment jsdom */
// The task page's route host. Thin by design — the surface is
// core/trackerTaskPage.js — so what is checked here is what the ROUTE owes it:
// the machine the project is on, its caller and its catalog, the feed, and a
// teardown that takes the surface with it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mountTaskPage = vi.fn(() => ({ feedMoved: vi.fn(), dispose: vi.fn() }));
const mountAgentRail = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));
vi.mock("../src/core/trackerTaskPage.js", () => ({ mountTaskPage: (...args) => mountTaskPage(...args) }));

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
  refreshFeed: async () => [],
  deliverFeed: () => {},
  dropFeedDevice: () => {},
  joinFeed: () => {},
}));

import { App } from "../src/app.js";
import { renderTrackerTask } from "../src/views/trackerTaskView.js";
import { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const device = (deviceId) => {
  const call = vi.fn(async () => ({}));
  // Greeted, too: a project with no owner is minted one only once its
  // bridge has said which API it speaks (core/shell.js).
  adoptBridgeSelection(adoptDeviceSession({ ...fakeSession(deviceId), call }), { version: "2.0.0" }, null);
  return call;
};

let elsewhere;

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML =
    '<div id="toolbar"><span id="tb-verb"></span></div><nav id="dir-rail"></nav><div id="root"></div><aside id="agent-rail"></aside>';
  mountTaskPage.mockClear();
  mountAgentRail.mockClear();
  subscribers = [];
  snapshot = { items: [], projects: [], workspaces: [], pending: [], devices: {} };
  App.viewDispose = null;
  App.viewingContext = { clear() {} };
  App.devices = [
    { id: "dev-1", name: "this machine", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  App.route = { name: "trackerTask", deviceId: "dev-1", projectId: "proj-1", taskId: "task-1" };
  device("dev-1");
  elsewhere = device("dev-2");
});

afterEach(() => {
  expect(elsewhere).not.toHaveBeenCalled();
  App.viewDispose?.();
  App.viewDispose = null;
  resetDeviceContexts();
});

describe("the task route", () => {
  // A task belongs to exactly one project and never moves between projects,
  // so the project in the URL is the project — there is nothing to look up.
  it("mounts the surface on the task and the project the URL names", async () => {
    await renderTrackerTask();
    await flush();
    const [host, given] = mountTaskPage.mock.calls[0];
    expect(host.id).toBe("task-pane");
    expect([given.projectId, given.deviceId, given.taskId, given.projectKey]).toEqual([
      "proj-1", "dev-1", "task-1", "dev-1/proj-1",
    ]);
  });

  // The rail beside a task is the SHELL's (core/shell.js stands it on the
  // project's conversation before this page paints), so this page mounts none
  // and takes none down. That is the point: the page that lost the bubble strip
  // lost it by being the one responsible for mounting it. Where the project's
  // owner comes from, and that a task of the project is the same standing as
  // the project page, are held in test/shellProjectRail.test.js.
  it("mounts no rail of its own, so it cannot forget one", async () => {
    const call = device("dev-1");
    await renderTrackerTask();
    await flush();
    expect(mountAgentRail).not.toHaveBeenCalled();
    expect(call).not.toHaveBeenCalledWith("project.ensure_conversation", expect.anything());
  });

  it("leaves the rail standing when the page goes, because it is not the page's", async () => {
    document.querySelector("#agent-rail").innerHTML = '<div class="rail-strip"></div>';
    await renderTrackerTask();
    await flush();
    App.viewDispose();
    App.viewDispose = null;
    expect(document.querySelector("#agent-rail .rail-strip")).not.toBeNull();
  });

  it("hands over the feed the links and the assignee names are read off", async () => {
    snapshot = { ...snapshot, projects: [{ id: "proj-1" }] };
    await renderTrackerTask();
    await flush();
    expect(mountTaskPage.mock.calls[0][1].feed().projects).toHaveLength(1);
  });

  it("repaints the surface when the feed moves, asking the bridge nothing", async () => {
    await renderTrackerTask();
    await flush();
    const page = mountTaskPage.mock.results[0].value;
    page.feedMoved.mockClear();
    subscribers.forEach((fn) => fn(snapshot));
    expect(page.feedMoved).toHaveBeenCalled();
  });

  // A machine that cannot answer has nothing under this link to read or write.
  it("names the machine rather than standing a page up over refused calls", async () => {
    App.route = { name: "trackerTask", deviceId: "dev-9", projectId: "proj-1", taskId: "task-1" };
    await renderTrackerTask();
    await flush();
    expect(mountTaskPage).not.toHaveBeenCalled();
    expect(document.querySelector("#root").textContent).not.toBe("");
  });

  it("takes the surface down with the route", async () => {
    await renderTrackerTask();
    await flush();
    const page = mountTaskPage.mock.results[0].value;
    App.viewDispose();
    App.viewDispose = null;
    expect(page.dispose).toHaveBeenCalled();
    expect(subscribers).toHaveLength(0);
  });

  // #274: a task is a page OF the project's Tasks face, so the project's rail
  // stands beside it with Tasks open, and is the way back to either list.
  it("stands the project's rail with Tasks open, each face going to that tab", async () => {
    await renderTrackerTask();
    await flush();
    const rail = document.querySelector("#dir-rail");
    const faces = [...rail.querySelectorAll("[data-tab]")].map((cell) => [cell.dataset.tab, cell.getAttribute("aria-selected")]);
    expect(faces).toEqual([["tasks", "true"], ["files", "false"], ["workspaces", "false"]]);
    expect(rail.querySelector("[data-rail-settings]")).not.toBeNull();
    rail.querySelector("[data-tab=workspaces]").click();
    expect(location.hash).toBe("#/device/dev-1/project/proj-1/workspaces");
    App.viewDispose();
    App.viewDispose = null;
    expect(rail.children).toHaveLength(0);
  });
});
