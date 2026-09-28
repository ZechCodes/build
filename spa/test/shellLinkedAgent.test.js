// @vitest-environment jsdom
// #200, Zech: "A chat notification should deeplink to and open the chat if
// necessary." A link naming an agent over the page already standing stands the
// rail again on that agent, panel out — whether the rail was collapsed or open
// on another agent, and even where the URL already named it (a notification
// clicked on the page it links to). Only a notification open lands on the
// latest message; every other link (a topic link, a reload, back/forward)
// lands on the unread line.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const collapse = vi.fn();
const dispose = vi.fn();
const mountAgentRail = vi.fn(() => ({ collapse, dispose }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));
const mountConsole = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/console.js", () => ({ mountConsole: (...args) => mountConsole(...args) }));

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn({ items: [], plans: [], runs: [], externalWorktrees: [], projects: [], workspaces: [], devices: {} });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
  deliverFeed: () => {},
  joinFeed: () => {},
  dropFeedDevice: () => {},
}));

const { askToOpenLinkedAgent, standShell, stopShell } = await import("../src/core/shell.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { App, followNotificationLink, initRouter } = await import("../src/app.js");
const { routeFromHash } = await import("../src/core/router.js");

const widthIs = (px) => Object.defineProperty(window, "innerWidth", { configurable: true, value: px });
const WORKSPACE = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "w-1", tab: "changes" };
const lastRail = () => mountAgentRail.mock.calls.at(-1)[1];

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = '<div id="agent-rail"></div><div id="console-region"></div><div id="root"></div>';
  vi.clearAllMocks();
  widthIs(1400);
  adoptDeviceSession({ deviceId: "dev-1", call: async () => ({}), close: () => {}, peer: () => {}, onCarrier: () => {} });
});

afterEach(() => {
  stopShell();
  resetDeviceContexts();
});

describe("a link naming an agent over the standing page", () => {
  it("stands the rail again on that agent, at its unread line, keeping the console", () => {
    standShell(WORKSPACE);
    expect(lastRail()).toMatchObject({ openAgentId: null, landOnLatest: false });
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    expect(lastRail()).toMatchObject({ kind: "workspace", workspaceId: "w-1", openAgentId: "agent-7", landOnLatest: false });
    expect(mountConsole).toHaveBeenCalledTimes(1);
  });

  it("lands a fresh stand on a URL naming an agent (a reload) on the unread line", () => {
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: false });
  });

  it("lands a topic link to another page on the unread line", () => {
    standShell(WORKSPACE);
    standShell({ ...WORKSPACE, workspaceId: "w-2", agent: "agent-3" });
    expect(lastRail()).toMatchObject({ openAgentId: "agent-3", landOnLatest: false });
  });

  it("moves the rail off another agent onto the one named", () => {
    standShell({ ...WORKSPACE, agent: "agent-1" });
    standShell({ ...WORKSPACE, agent: "agent-2" });
    expect(lastRail().openAgentId).toBe("agent-2");
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
  });

  it("reopens on a phone, after the navigation put the chat away", () => {
    widthIs(390);
    standShell(WORKSPACE);
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(collapse).toHaveBeenCalledTimes(1);
    expect(lastRail().openAgentId).toBe("agent-7");
  });

  it("reopens the agent the URL already names when a notification asks", () => {
    standShell({ ...WORKSPACE, agent: "agent-7" });
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    askToOpenLinkedAgent();
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: true });
    // Asked once: the next paint of the same page leaves the rail alone.
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    // And the next link lands on the unread line again.
    standShell({ ...WORKSPACE, agent: "agent-8" });
    expect(lastRail()).toMatchObject({ openAgentId: "agent-8", landOnLatest: false });
  });

  it("leaves the rail alone on a tab change that keeps the same agent", () => {
    standShell({ ...WORKSPACE, agent: "agent-7" });
    standShell({ ...WORKSPACE, tab: "files", agent: "agent-7" });
    standShell({ ...WORKSPACE, tab: "files" });
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(dispose).not.toHaveBeenCalled();
  });
});

describe("following a notification link in the open window", () => {
  it("stands the rail again when the link is the page already open", () => {
    const hash = "#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7";
    history.replaceState(null, "", hash);
    App.route = routeFromHash(hash);
    App.gated = false;
    standShell(App.route);
    followNotificationLink(hash);
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: true });
  });

  it("routes to another page through the hash, which the router renders", () => {
    history.replaceState(null, "", "#/device/dev-1/project/p-1/workspace/w-1/changes");
    followNotificationLink("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7");
    expect(location.hash).toBe("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7");
  });

  it("takes the service worker's mark off the link, and lands the stand it causes on the latest message", () => {
    history.replaceState(null, "", "#/device/dev-1/project/p-1/workspace/w-1/changes");
    App.route = routeFromHash(location.hash);
    standShell(App.route);
    followNotificationLink("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7&from=push");
    expect(location.hash).toBe("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7");
    // The router's hashchange stands the page the hash now names.
    standShell(routeFromHash(location.hash));
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: true });
    // One-shot: a later link lands on the unread line.
    standShell(routeFromHash("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-8"));
    expect(lastRail()).toMatchObject({ openAgentId: "agent-8", landOnLatest: false });
  });

  it("takes the mark off a marked link to the page already open, and lands on the latest message", () => {
    const hash = "#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7";
    history.replaceState(null, "", hash);
    App.route = routeFromHash(hash);
    App.gated = false;
    standShell(App.route);
    followNotificationLink(`${hash}&from=push`);
    expect(location.hash).toBe(hash);
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: true });
  });
});

describe("a cold start on a notification's link", () => {
  it("lands the first stand on the latest message and drops the mark from the URL, without a new entry", () => {
    const length = history.length;
    history.replaceState(null, "", "#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7&from=push");
    initRouter();
    expect(location.hash).toBe("#/device/dev-1/project/p-1/workspace/w-1/changes?agent=agent-7");
    expect(history.length).toBe(length);
    expect(App.route).toMatchObject({ name: "workspace", workspaceId: "w-1", agent: "agent-7" });
    expect(App.route).not.toHaveProperty("fromPush");
    standShell(App.route);
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: true });
    // A reload of the URL left behind lands on the unread line.
    stopShell();
    standShell(routeFromHash(location.hash));
    expect(lastRail()).toMatchObject({ openAgentId: "agent-7", landOnLatest: false });
  });
});
