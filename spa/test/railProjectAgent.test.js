// @vitest-environment jsdom
// The project's agent on a workspace's rail.
//
// "It needs to be accessible from all workspaces in the project, hence project
// agent. It should be there even if it hasn't been started yet." So the bubble
// is on every workspace's strip above a line, whether or not the project has a
// conversation yet — and pressing it puts that conversation in the SAME panel,
// with the page still standing on the workspace.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const feedSubscribers = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (listener) => {
    feedSubscribers.add(listener);
    listener({ items: [], projects: [], workspaces: [] });
    return () => feedSubscribers.delete(listener);
  },
  refreshFeed: async () => {},
  dropFeedDevice: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({ markSeen: async () => {} }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));
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
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { wipeCache } = await import("../src/core/localCache.js");

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }],
};

const DEVICE_ID = "device-1";
const PROJECT_ID = "proj-1";
const WORKSPACE_ID = "ws-1";
const OWNER = "run-project";

/** The topic each agent named its work with: the head and the tips say the
 *  topic, so it is how these cases pin which conversation the panel is on. */
const agent = (id, over = {}) => ({
  id,
  ordinal: 1,
  topic: id === "pa-1" ? "Sort the workspaces" : "Fix login redirect",
  conversation_id: `conversation-${id}`,
  provider: "claude_adk",
  state: "live",
  unread_count: 0,
  working: false,
  ...over,
});

const workspacePayload = () => ({
  workspace_id: WORKSPACE_ID,
  project_id: PROJECT_ID,
  entity_id: "run-workspace",
  agents: [agent("wa-1")],
  directories: [],
  thread: { items: [], sessions: [] },
});

const projectPayload = (over = {}) => ({
  entity_id: OWNER,
  run_id: OWNER,
  project_id: PROJECT_ID,
  agents: [agent("pa-1")],
  thread: { items: [], sessions: [] },
  ...over,
});

const flush = async () => {
  for (let count = 0; count < 8; count += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const host = () => document.querySelector("#agent-rail");
const strip = () => [...host().querySelectorAll(".rail-strip > *")];
const projectBubble = () => host().querySelector('[data-bubble="project"]');
const agentBubble = (id) => host().querySelector(`[data-bubble="agent"][data-agent="${id}"]`);
/** Whose conversation the panel's head says is open. */
const headWho = () => host().querySelector(".rail-who")?.title || "";
const panel = () => host().querySelector("#rail-panel");
/** Whether the panel is on screen — which, unpinned, is the card being out. */
const panelIsOpen = () => panel()?.getAttribute("aria-hidden") === "false";
const callsTo = (method) => calls.filter((call) => call.method === method);

let workspace;
let project;
let owner;
let calls;
let rail;

const mountWorkspaceRail = async () => {
  rail = mountAgentRail(host(), {
    kind: "workspace",
    deviceId: DEVICE_ID,
    workspaceId: WORKSPACE_ID,
    projectId: PROJECT_ID,
    projectAgent: { projectId: PROJECT_ID },
    cacheScope: contextFor(DEVICE_ID).cacheScope,
    chatRepository: contextFor(DEVICE_ID).chatRepository,
  });
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  feedSubscribers.clear();
  notifyError.mockClear();
  workspace = workspacePayload();
  project = projectPayload();
  owner = null; // the project has no conversation until something mints one
  calls = [];
  adoptDeviceSession({
    deviceId: DEVICE_ID,
    call: async (method, params = {}) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return workspace;
      if (method === "project.list") {
        return { projects: [{ project_id: PROJECT_ID, name: "build", entity_id: owner, run_id: owner }] };
      }
      if (method === "project.ensure_conversation") {
        owner = OWNER;
        return { project_id: PROJECT_ID, entity_id: OWNER, run_id: OWNER };
      }
      if (method === "run.get") return project;
      return {};
    },
  });
  await wipeCache();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
});

afterEach(() => {
  rail?.dispose();
  rail = null;
  resetApplication();
  resetChangeEvents();
  vi.useRealTimers();
});

describe("the project's agent on a workspace's strip", () => {
  it("stands one bubble above a line, with the workspace's own agents below", async () => {
    await mountWorkspaceRail();

    expect(strip().map((element) => element.dataset.bubble || element.className)).toEqual([
      "project",
      "rail-sep",
      "agent",
      "add",
    ]);
    expect(projectBubble().querySelector(".rail-bubble-label").textContent).toBe("B");
  });

  it("is there before the project has a conversation, and says how to start it", async () => {
    await mountWorkspaceRail();

    expect(projectBubble().title).toBe("Project agent for build: send a message to start it");
    // Nothing is minted to paint it. A rail that asked for an owner would give
    // every workspace page a project agent nobody asked for.
    expect(callsTo("project.ensure_conversation")).toHaveLength(0);
    expect(callsTo("project.list")).toHaveLength(1);
  });

  it("carries what the project's agent is waiting on once there is one", async () => {
    owner = OWNER;
    project = projectPayload({ agents: [agent("pa-1", { unread_count: 3, working: true })] });
    await mountWorkspaceRail();

    expect(projectBubble().title).toBe("Project agent for build — 3 unread");
    expect(projectBubble().querySelector(".rail-count").textContent).toBe("3");
    expect(projectBubble().classList.contains("working")).toBe(true);
  });
});

describe("pressing the project's agent", () => {
  it("mints the conversation once and shows it in the same panel", async () => {
    await mountWorkspaceRail();
    expect(headWho()).toBe("Fix login redirect");

    projectBubble().click();
    await flush();

    expect(callsTo("project.ensure_conversation")).toHaveLength(1);
    expect(headWho()).toBe("Sort the workspaces");
    // The page has not moved: the workspace's own agents are still on the strip
    // below the line, and the project's bubble is the open one.
    expect(projectBubble().classList.contains("active")).toBe(true);
    expect(agentBubble("wa-1")).toBeTruthy();
    expect(agentBubble("wa-1").classList.contains("active")).toBe(false);
  });

  it("swaps back to the workspace's agent, minting nothing a second time", async () => {
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    agentBubble("wa-1").click();
    await flush();

    expect(headWho()).toBe("Fix login redirect");
    expect(agentBubble("wa-1").classList.contains("active")).toBe(true);
    expect(projectBubble().classList.contains("active")).toBe(false);
    // The owner was minted by the first press and carried across both swaps.
    expect(callsTo("project.ensure_conversation")).toHaveLength(1);
    expect(projectBubble().title).toBe("Project agent for build");
  });
});

// The panel unpinned is a card on the strip, and the swap between the two
// conversations is a re-mount of the same host. Neither is the reader's
// business: pressing a bubble across the line has to land exactly where
// pressing one below it lands — the panel open, on the conversation pressed,
// with the strip still saying what both sides are doing.
describe("swapping with the panel unpinned", () => {
  const atWidth = (width) => Object.defineProperty(window, "innerWidth", { configurable: true, value: width });

  beforeEach(() => atWidth(390));
  afterEach(() => atWidth(1024));

  it("keeps the open panel open, on the project's conversation", async () => {
    await mountWorkspaceRail();
    agentBubble("wa-1").click();
    await flush();
    expect(panelIsOpen()).toBe(true);

    projectBubble().click();
    await flush();

    expect(panelIsOpen()).toBe(true);
    expect(headWho()).toBe("Sort the workspaces");
  });

  it("keeps it open on the way back to the workspace's own", async () => {
    await mountWorkspaceRail();
    agentBubble("wa-1").click();
    await flush();
    projectBubble().click();
    await flush();

    agentBubble("wa-1").click();
    await flush();

    expect(panelIsOpen()).toBe(true);
    expect(headWho()).toBe("Fix login redirect");
  });

  it("opens the panel a closed card was left at, the way any other bubble does", async () => {
    await mountWorkspaceRail();
    expect(panelIsOpen()).toBe(false);

    projectBubble().click();
    await flush();

    expect(panelIsOpen()).toBe(true);
    expect(headWho()).toBe("Sort the workspaces");
  });
});

// The rail hosts one conversation and reads the other beside it, so it already
// knows what both sides are. A swap must paint from that: a strip that starts
// again from nothing reads as a workspace with no conversations at all, and
// then fills in — which is the workspace's agents blinking out under a press
// that was only meant to change which one is open.
describe("the strip across a swap", () => {
  it("paints the workspace's agents on the first frame after the swap back", async () => {
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    agentBubble("wa-1").click();

    // No flush: this is what the strip says before any read answers.
    expect(strip().map((element) => element.dataset.bubble || element.className)).toEqual([
      "project",
      "rail-sep",
      "agent",
      "add",
    ]);
    expect(host().querySelector('[data-bubble="ghost"]')).toBeNull();
    expect(agentBubble("wa-1").classList.contains("active")).toBe(true);
  });

  it("carries what each side is doing across, so neither goes quiet mid-swap", async () => {
    owner = OWNER;
    workspace = { ...workspacePayload(), agents: [agent("wa-1", { unread_count: 2, working: true })] };
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    agentBubble("wa-1").click();

    expect(agentBubble("wa-1").querySelector(".rail-count").textContent).toBe("2");
    expect(agentBubble("wa-1").classList.contains("working")).toBe(true);
    // …and the project's bubble, now the one beside, still says its own.
    expect(projectBubble().title).toBe("Project agent for build");
  });
});
