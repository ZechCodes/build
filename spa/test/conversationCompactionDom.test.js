// @vitest-environment jsdom
// When a conversation compacts, chosen on the ⋮ of its head.
//
// Nothing here is mocked that the choice passes through: the bridge greets
// through the real greeting path, so the gate is the adapter's own; the verb
// goes out through the device's real `call`; and a refusal is read off the
// notices the real notify module draws.
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

/** The agent's compaction as its digest carries it; a case sets its own. */
let digestCompaction = { max_context_tokens: null, compact_at_tokens: 200000 };

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
      last_context_tokens: 120000,
      ...digestCompaction,
    },
  ],
  directories: [],
  thread: { items: [], sessions: [] },
});

/** What `conversation.settings` answers, or throws; a case replaces it. */
let answerSettings;
let settingsAsked;

const machineCall = async (method, params) => {
  if (method === "conversation.settings") {
    settingsAsked.push(params);
    return answerSettings(params);
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
const menuCaret = () => panel().querySelector(".rail-surface-menu .caret");
const compactionRows = () => [...panel().querySelectorAll('.rail-surface-menu .mi[data-action^="compact:"]')];
const markedRow = () => compactionRows().find((row) => row.classList.contains("on"));
const rowLabel = (row) => row?.querySelector(".mt").textContent;
const errorNotices = () => [...document.querySelectorAll("#notices .notice")].map((notice) => notice.textContent);

let rail;

const mountWorkspaceRail = async (apiVersion = "1.10.0") => {
  await greetBridge(async () => ({ push_events: true, api_version: apiVersion }), { deviceId: DEVICE_ID });
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

const choose = async (optionId) => {
  menuCaret().click();
  panel().querySelector(`.rail-surface-menu .mi[data-action="${optionId}"]`).click();
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  await wipeCache();
  digestCompaction = { max_context_tokens: null, compact_at_tokens: 200000 };
  settingsAsked = [];
  answerSettings = ({ agent_id, max_context_tokens }) => ({
    agent_id,
    max_context_tokens,
    compact_at_tokens: max_context_tokens ?? 200000,
  });
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

describe("the context gauge beside the paperclip", () => {
  const gauge = () => panel().querySelector("#railinputgauge");

  it("shows how near the chat is to compacting, off the digest", async () => {
    await mountWorkspaceRail();

    expect(gauge().hidden).toBe(false);
    expect(gauge().textContent).toBe("60%");
    expect(gauge().dataset.step).toBe("accent");
    expect(gauge().title).toBe("120k of 200k tokens (compacts at 200k), as of its last turn");
  });

  it("measures a chat that never compacts against the model's window", async () => {
    digestCompaction = { max_context_tokens: 0, compact_at_tokens: 0 };
    await mountWorkspaceRail();

    expect(gauge().textContent).toBe("12%");
    expect(gauge().dataset.step).toBe("dim");
    expect(gauge().title).toBe("120k of 1M window, as of its last turn");
  });

  it("moves with a new record and leaves the box being typed in alone", async () => {
    await mountWorkspaceRail();
    const input = panel().querySelector("#railinput");
    input.value = "half a sentence";
    input.focus();
    input.setSelectionRange(4, 4);

    digestCompaction = { ...digestCompaction, last_context_tokens: 190000 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();

    expect(gauge().textContent).toBe("95%");
    expect(gauge().dataset.step).toBe("warning");
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(input.selectionStart).toBe(4);
  });

  it("takes no space for an agent that has not run a turn", async () => {
    digestCompaction = { ...digestCompaction, last_context_tokens: null };
    await mountWorkspaceRail();

    expect(gauge().hidden).toBe(true);
  });
});

describe("compaction on the conversation's menu", () => {
  it("shows the device default the digest names, with its threshold", async () => {
    await mountWorkspaceRail();

    expect(compactionRows().map((row) => row.dataset.action)).toEqual([
      "compact:default",
      "compact:150000",
      "compact:200000",
      "compact:300000",
      "compact:off",
    ]);
    expect(rowLabel(markedRow())).toBe("Compact at: Default (200k)");
  });

  it("shows the conversation's own limit when the digest carries one", async () => {
    digestCompaction = { max_context_tokens: 300000, compact_at_tokens: 300000 };
    await mountWorkspaceRail();

    expect(markedRow().dataset.action).toBe("compact:300000");
  });

  it("is not offered by a bridge that predates conversation.settings", async () => {
    await mountWorkspaceRail("1.9.0");

    expect(menuCaret()).not.toBe(null);
    expect(compactionRows()).toEqual([]);
  });

  it.each([
    ["compact:150000", 150000],
    ["compact:200000", 200000],
    ["compact:300000", 300000],
    ["compact:off", 0],
  ])("sends %s as max_context_tokens %s", async (optionId, tokens) => {
    await mountWorkspaceRail();

    await choose(optionId);

    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: tokens }]);
  });

  it("sends null to go back to the device default", async () => {
    digestCompaction = { max_context_tokens: 150000, compact_at_tokens: 150000 };
    await mountWorkspaceRail();

    await choose("compact:default");

    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: null }]);
    expect(rowLabel(markedRow())).toBe("Compact at: Default (200k)");
  });

  it("marks what the bridge answered", async () => {
    await mountWorkspaceRail();

    await choose("compact:off");

    expect(markedRow().dataset.action).toBe("compact:off");
  });

  it("says a refusal in a sentence and leaves the choice where it was", async () => {
    answerSettings = () => {
      throw new Error("agent wa-1 is not on run-workspace");
    };
    await mountWorkspaceRail();

    await choose("compact:150000");

    expect(errorNotices().some((text) => text.includes("Build could not change when this chat compacts."))).toBe(true);
    expect(markedRow().dataset.action).toBe("compact:default");
  });
});
