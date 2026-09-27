// @vitest-environment jsdom
// #62. The maintainer, on their phone with an agent's chat open on a workspace
// page: "On mobile, where the chat takes up the full width of the viewport,
// tapping a tab should collapse the chat. So if I'm looking at this chat and
// then tap 'Tasks' it should collapse so I can see the tasks." And then:
// "That needs to go for deep links too. Very confusing that tapping them
// doesn't collapse the chat."
//
// So the rule is not about tabs. At a phone width the chat is laid OVER the
// page, and ANY navigation from inside it changes a page the reader cannot
// see. Beside the page — a desktop — it takes nothing away, so it stays.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

const collapse = vi.fn();
const dispose = vi.fn();
const mountAgentRail = vi.fn(() => ({ collapse, dispose }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));

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

const { chatOverlaysPage } = await import("../src/core/railLayout.js");
const { collapseChatOverPage, standShell, stopShell } = await import("../src/core/shell.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");

const PHONE = 390;
const DESKTOP = 1400;
const widthIs = (px) => Object.defineProperty(window, "innerWidth", { configurable: true, value: px });

const WORKSPACE = { name: "workspace", deviceId: "dev-1", projectId: "p-1", workspaceId: "w-1" };
const TASK = { name: "trackerTask", deviceId: "dev-1", projectId: "p-1", taskId: "i-1" };

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = '<div id="agent-rail"></div><div id="console-region"></div><div id="root"></div>';
  collapse.mockClear();
  dispose.mockClear();
  mountAgentRail.mockClear();
  adoptDeviceSession({ deviceId: "dev-1", call: async () => ({}), close: () => {}, peer: () => {}, onCarrier: () => {} });
});

afterEach(() => {
  stopShell();
  resetDeviceContexts();
});

describe("where the chat is", () => {
  // The same breakpoint the stylesheet lays the panel over the page at, named
  // once beside it (styles/shell.css `@media (max-width: 760px)`).
  it("is over the page on a phone and beside it on a desktop", () => {
    for (const [width, over] of [[PHONE, true], [760, true], [761, false], [DESKTOP, false]]) {
      widthIs(width);
      expect([width, chatOverlaysPage()]).toEqual([width, over]);
    }
  });
});

describe("a navigation with the chat open", () => {
  it("puts the chat away on a phone", () => {
    widthIs(PHONE);
    standShell(WORKSPACE);
    expect(collapse).not.toHaveBeenCalled(); // arriving is not moving
    standShell(TASK);
    expect(collapse).toHaveBeenCalledTimes(1);
  });

  it("leaves it alone on a desktop, where it covers nothing", () => {
    widthIs(DESKTOP);
    standShell(WORKSPACE);
    standShell(TASK);
    expect(collapse).not.toHaveBeenCalled();
  });

  // A deep link out of the conversation — a task line, an action line, a
  // link the markdown renderer produced. The rule is at the route change, so a
  // link nobody thought of behaves like the rest.
  it("puts it away for a deep link, not only a tab", () => {
    widthIs(PHONE);
    standShell(WORKSPACE);
    standShell({ ...TASK, taskId: "i-9" });
    expect(collapse).toHaveBeenCalledTimes(1);
  });

  // The bubbles are the shell's and the rail is kept, so the same conversation
  // is one press away at the place the reader left it.
  it("keeps the rail standing, so the bubble reopens the same chat", () => {
    widthIs(PHONE);
    standShell({ ...WORKSPACE, tab: "changes" });
    const mounts = mountAgentRail.mock.calls.length;
    standShell({ ...WORKSPACE, tab: "files" }); // same workspace, same conversation
    expect(collapse).toHaveBeenCalledTimes(1);
    expect(dispose).not.toHaveBeenCalled();
    expect(mountAgentRail.mock.calls.length).toBe(mounts);
  });

  it("does nothing when the page paints again without moving", () => {
    widthIs(PHONE);
    standShell(WORKSPACE);
    standShell({ ...WORKSPACE });
    expect(collapse).not.toHaveBeenCalled();
  });

  // A URL that names a conversation is ASKING for the panel. Collapsing runs
  // before the new standing is mounted, so the link still opens what it named.
  it("leaves a link that names an agent to open its own panel", () => {
    widthIs(PHONE);
    standShell(WORKSPACE);
    standShell({ ...WORKSPACE, workspaceId: "w-2", agent: "agent-7" });
    expect(mountAgentRail.mock.calls.at(-1)[1].openAgentId).toBe("agent-7");
  });
});

describe("the collapse itself", () => {
  it("asks nothing of a route that stands on no conversation", () => {
    widthIs(PHONE);
    standShell({ name: "inbox" });
    expect(collapseChatOverPage()).toBe(true);
    expect(collapse).not.toHaveBeenCalled();
  });

  it("says whether the chat was over the page, which is what a caller acts on", () => {
    widthIs(PHONE);
    standShell(WORKSPACE);
    expect(collapseChatOverPage()).toBe(true);
    widthIs(DESKTOP);
    expect(collapseChatOverPage()).toBe(false);
  });
});
