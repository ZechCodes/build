// @vitest-environment jsdom
// #200, Zech: "A chat notification should deeplink to and open the chat if
// necessary." A link naming an agent over the page already standing stands the
// rail again on that agent, panel out, landing on the latest message — whether
// the rail was collapsed or open on another agent, and even where the URL
// already named it (a notification clicked on the page it links to).

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
const { App, followNotificationLink } = await import("../src/app.js");
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
  it("stands the rail again on that agent, at its latest message, keeping the console", () => {
    standShell(WORKSPACE);
    expect(lastRail()).toMatchObject({ openAgentId: null, landOnLatest: false });
    standShell({ ...WORKSPACE, agent: "agent-7" });
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(mountAgentRail).toHaveBeenCalledTimes(2);
    expect(lastRail()).toMatchObject({ kind: "workspace", workspaceId: "w-1", openAgentId: "agent-7", landOnLatest: true });
    expect(mountConsole).toHaveBeenCalledTimes(1);
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
});
