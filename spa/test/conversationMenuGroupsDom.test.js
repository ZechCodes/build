// @vitest-environment jsdom
// The conversation head's ⋮ in sections (#124): what the agent opened, then
// the two settings the conversation carries, each under its own name.
//
// Nothing the menu passes through is stood in for: the production rail is
// mounted on a row the sync layer would have written, the bridge is greeted
// through the real greeting path (which is what makes the compaction rows
// appear), the real menu markup is mounted and wired, and a pick is read off
// what it changed — the timeline, the overlay, the verb sent.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (listener) => {
    listener({ items: [], projects: [], workspaces: [] });
    return () => {};
  },
  refreshFeed: async () => [],
  dropFeedDevice: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({ markSeen: async () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose() {} }) }));
vi.mock("../src/core/agentCanvas.js", () => ({
  createPatternRenderer: () => ({
    destroy() {},
    isWorking: () => false,
    setDimmed() {},
    setInk() {},
    setWorking() {},
  }),
}));

const { resetApplication } = await import("../src/app.js");
const { adoptDeviceSession, contextFor } = await import("../src/core/deviceContexts.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { dismissAllNotices } = await import("../src/core/notify.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { writeRailBoard, writeRailWorkItem } = await import("./railCacheFixture.js");

const DEVICE_ID = "device-1";
const PROJECT_ID = "proj-1";
const WORKSPACE_ID = "ws-1";
const WORKSPACE_OWNER = "run-workspace";

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }],
};

/** What the agent has opened; a case hands over its own, or none. */
let surfaces = {
  shells: [{ id: "sh1", description: "cargo test", state: "running", tail: [] }],
  checklist: [
    { id: "c1", subject: "Read the menu", state: "completed" },
    { id: "c2", subject: "Group it", state: "in_progress" },
  ],
};

/** A conversation with something at every level: the reader, the agent, and
 *  a tool call between them. */
const CONVERSATION = [
  { type: "message", data: { id: "m1", sequence: 1, role: "user", body: "look at the retry path" } },
  { type: "tool_use", data: { sequence: 2, event: "tool_use", summary: "read src/retry.rs" } },
  { type: "message", data: { id: "m3", sequence: 3, role: "agent", body: "the retry is fixed" } },
];

const workspacePayload = () => ({
  kind: "branch",
  workspace_id: WORKSPACE_ID,
  project_id: PROJECT_ID,
  entity_id: WORKSPACE_OWNER,
  agents: [
    {
      id: "wa-1",
      ordinal: 1,
      topic: "Fix the retry",
      conversation_id: "conversation-wa-1",
      provider: "claude_adk",
      state: "live",
      unread_count: 0,
      working: false,
      max_context_tokens: null,
      compact_at_tokens: 200000,
      surface_session_generation: "session-one",
      surfaces,
    },
  ],
  directories: [],
  thread: { items: CONVERSATION, sessions: [] },
});

let settingsAsked;

const machineCall = async (method, params) => {
  if (method === "conversation.settings") {
    settingsAsked.push(params);
    return { agent_id: params.agent_id, max_context_tokens: params.max_context_tokens, compact_at_tokens: params.max_context_tokens ?? 200000 };
  }
  if (method === "models.list") return CATALOG;
  if (method === "workspace.get") return workspacePayload();
  return {};
};

const flush = async () => {
  for (let count = 0; count < 16; count += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const host = () => document.querySelector("#agent-rail");
const panel = () => host().querySelector("#rail-panel");
const menu = () => panel().querySelector(".rail-surface-menu .splitmenu");
const menuCaret = () => panel().querySelector(".rail-surface-menu .caret");
const groups = () => [...menu().querySelectorAll('[role="group"]')];
const groupNamed = (label) => groups().find((group) => group.getAttribute("aria-label") === label) || null;
const rowsOf = (group) => [...group.querySelectorAll(".mi")];
const rowLabel = (row) => row.querySelector(".mt").textContent;
const checkedIn = (group) => rowsOf(group).filter((row) => row.getAttribute("aria-checked") === "true").map((row) => row.dataset.action);
const row = (action) => panel().querySelector(`.rail-surface-menu .mi[data-action="${action}"]`);
const overlay = () => document.querySelector(".modal-surface");
const activityRows = () => panel().querySelectorAll(".thread-activity-group, .thread-activity").length;

let rail;

const mountWorkspaceRail = async () => {
  await greetBridge(async () => ({ push_events: true, api_version: "2.0.0", capabilities: ["conversations.settings"] }), { deviceId: DEVICE_ID });
  await writeRailBoard({
    projects: [{ project_id: PROJECT_ID, name: "build" }],
    workspaces: [{ id: WORKSPACE_ID, project_id: PROJECT_ID, name: "login", status: "ready", entity_id: WORKSPACE_OWNER }],
  }, { deviceId: DEVICE_ID });
  await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
  rail = mountAgentRail(host(), {
    kind: "workspace",
    deviceId: DEVICE_ID,
    workspaceId: WORKSPACE_ID,
    projectId: PROJECT_ID,
    cacheScope: contextFor(DEVICE_ID).cacheScope,
    chatRepository: contextFor(DEVICE_ID).chatRepository,
  });
  await flush();
};

const keydown = (target, key) => target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  await wipeCache();
  settingsAsked = [];
  surfaces = {
    shells: [{ id: "sh1", description: "cargo test", state: "running", tail: [] }],
    checklist: [
      { id: "c1", subject: "Read the menu", state: "completed" },
      { id: "c2", subject: "Group it", state: "in_progress" },
    ],
  };
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  adoptDeviceSession({ deviceId: DEVICE_ID, call: machineCall });
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
});

afterEach(() => {
  rail?.dispose();
  rail = null;
  dismissAllNotices();
  resetApplication();
  resetChangeEvents();
  vi.useRealTimers();
});

describe("the sections of the conversation's menu", () => {
  it("offers what the agent opened, then the two settings, each under its own name", async () => {
    await mountWorkspaceRail();

    expect(menu().getAttribute("role")).toBe("menu");
    expect(groups().map((group) => group.getAttribute("aria-label"))).toEqual(["Show", "Detail", "Compact at"]);
    expect(rowsOf(groupNamed("Show")).map((each) => [each.dataset.action, rowLabel(each), each.querySelector(".md").textContent])).toEqual([
      ["shells", "Shells", "1 running"],
      ["checklist", "Checklist", "1/2 completed"],
    ]);
    expect(rowsOf(groupNamed("Detail")).map(rowLabel)).toEqual(["All", "All messages", "Agent only"]);
    expect(rowsOf(groupNamed("Compact at"))).toEqual([]);
    expect([...groupNamed("Compact at").querySelectorAll(".mt")].map((each) => each.textContent)).toEqual(["Default (200k)"]);
  });

  it("reads a surface as a thing to open and a setting as one answer of a set", async () => {
    await mountWorkspaceRail();

    expect(rowsOf(groupNamed("Show")).every((each) => each.getAttribute("role") === "menuitem")).toBe(true);
    expect(rowsOf(groupNamed("Detail")).every((each) => each.getAttribute("role") === "menuitemradio")).toBe(true);
    expect(groupNamed("Compact at").querySelectorAll('[role="slider"]')).toHaveLength(1);
    expect(checkedIn(groupNamed("Detail"))).toEqual(["detail:all"]);
    expect(groupNamed("Compact at").querySelector('[role="slider"]').dataset.action).toBe("compact:default");
  });

  it("leaves the Show group out when the agent has opened nothing, and keeps the settings", async () => {
    surfaces = null;
    await mountWorkspaceRail();

    expect(menuCaret()).not.toBe(null);
    expect(groupNamed("Show")).toBe(null);
    expect(groups().map((group) => group.getAttribute("aria-label"))).toEqual(["Detail", "Compact at"]);
  });
});

describe("a pick on the sectioned menu", () => {
  it("moves the mark and redraws the thread when it is a detail level", async () => {
    await mountWorkspaceRail();
    expect(activityRows()).toBe(1);

    menuCaret().click();
    row("detail:agent").click();
    await vi.waitFor(() => expect(checkedIn(groupNamed("Detail"))).toEqual(["detail:agent"]));

    expect(activityRows()).toBe(0);
    expect(row("detail:all").getAttribute("aria-checked")).toBe("false");
  });

  it("opens the overlay when it is a surface", async () => {
    await mountWorkspaceRail();

    menuCaret().click();
    row("shells").click();
    await vi.waitFor(() => expect(overlay()).not.toBe(null));

    expect(overlay().querySelector("h3").textContent).toBe("Shells");
  });

  it("sends the limit when the compaction slider changes", async () => {
    await mountWorkspaceRail();

    menuCaret().click();
    const slider = groupNamed("Compact at").querySelector('[role="slider"]');
    slider.value = "1";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
    slider.focus();
    keydown(slider, "Enter");
    await flush();

    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: 150000 }]);
    await vi.waitFor(() => expect(groupNamed("Compact at").querySelector('[role="slider"]').dataset.action).toBe("compact:150000"));
  });

  it("can be made from the keyboard, opener to row to overlay", async () => {
    await mountWorkspaceRail();
    menuCaret().focus();

    keydown(menuCaret(), "ArrowDown");
    expect(document.activeElement.dataset.action).toBe("shells");
    keydown(menu(), "Enter");
    await vi.waitFor(() => expect(overlay()).not.toBe(null));

    expect(overlay().querySelector("h3").textContent).toBe("Shells");
  });
});
