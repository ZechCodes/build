// @vitest-environment jsdom
// When a conversation compacts, chosen on the ⋮ of its head.
//
// Nothing here is mocked that the choice passes through: the bridge greets
// through the real greeting path where needed; the verb
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
const { adoptDeviceSession, contextFor, retireDeviceContext } = await import("../src/core/deviceContexts.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { dismissAllNotices } = await import("../src/core/notify.js");
const { readCached, wipeCache } = await import("../src/core/localCache.js");
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
const cachedAgent = async () =>
  (await readCached({ deviceId: DEVICE_ID, entityId: WORKSPACE_OWNER, kind: "row", sub: "" }))?.value.agents[0];
const errorNotices = () => [...document.querySelectorAll("#notices .notice")].map((notice) => notice.textContent);

let rail;

/** What a bridge that takes conversation.settings names in its greeting. */
const SETTINGS_CAPABILITIES = ["conversations.settings"];

const mountWorkspaceRail = async (capabilities = SETTINGS_CAPABILITIES) => {
  if (capabilities !== null) {
    await greetBridge(async () => ({ push_events: true, api_version: "2.0.0", capabilities }), { deviceId: DEVICE_ID });
  }
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
    expect(gauge().textContent).toBe("120k");
    expect(gauge().dataset.step).toBe("accent");
    expect(gauge().title).toBe("120k of 200k tokens (compacts at 200k), 60% used, as of its last turn");
  });

  it("measures a chat that never compacts against the model's window", async () => {
    digestCompaction = { max_context_tokens: 0, compact_at_tokens: 0 };
    await mountWorkspaceRail();

    expect(gauge().textContent).toBe("120k");
    expect(gauge().dataset.step).toBe("dim");
    expect(gauge().title).toBe("120k of 1M window, 12% used, as of its last turn");
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

    expect(gauge().textContent).toBe("190k");
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
    expect(rowLabel(markedRow())).toBe("Default (200k)");
    // The group says what the setting is; the rows are its values.
    expect(compactionRows()[0].closest('[role="group"]').getAttribute("aria-label")).toBe("Compact at");
    expect(compactionRows().every((row) => row.getAttribute("role") === "menuitemradio")).toBe(true);
  });

  it("shows the conversation's own limit when the digest carries one", async () => {
    digestCompaction = { max_context_tokens: 300000, compact_at_tokens: 300000 };
    await mountWorkspaceRail();

    expect(markedRow().dataset.action).toBe("compact:300000");
  });

  it("checks a limit set elsewhere on a row of its own, with its value", async () => {
    digestCompaction = { max_context_tokens: 250000, compact_at_tokens: 250000 };
    await mountWorkspaceRail();

    expect(compactionRows().map((row) => row.dataset.action)).toEqual([
      "compact:default",
      "compact:150000",
      "compact:200000",
      "compact:300000",
      "compact:250000",
      "compact:off",
    ]);
    expect(compactionRows().filter((row) => row.getAttribute("aria-checked") === "true")).toEqual([markedRow()]);
    expect(markedRow().dataset.action).toBe("compact:250000");
    expect(rowLabel(markedRow())).toBe("Custom (250k)");
  });

  it("sends nothing for the custom row already standing, and leaves it for an offered size", async () => {
    digestCompaction = { max_context_tokens: 250000, compact_at_tokens: 250000 };
    await mountWorkspaceRail();

    await choose("compact:250000");
    expect(settingsAsked).toEqual([]);
    // A row this menu drew never falls through to opening a surface.
    expect(document.querySelector(".modal-scrim")).toBe(null);

    await choose("compact:150000");
    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: 150000 }]);
    expect(markedRow().dataset.action).toBe("compact:150000");
    expect(compactionRows().map((row) => row.dataset.action)).not.toContain("compact:250000");
  });

  it("shows the digest's compaction choice before greeting", async () => {
    digestCompaction = { max_context_tokens: 300000, compact_at_tokens: 300000 };
    await mountWorkspaceRail(null);

    expect(markedRow().dataset.action).toBe("compact:300000");
  });

  it("keeps the compaction rows when a bridge's greeting does not name conversations.settings", async () => {
    await mountWorkspaceRail([]);

    expect(menuCaret()).not.toBe(null);
    expect(compactionRows()).toHaveLength(5);
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
    expect(rowLabel(markedRow())).toBe("Default (200k)");
  });

  it("marks what the bridge answered", async () => {
    await mountWorkspaceRail();

    await choose("compact:off");

    expect(markedRow().dataset.action).toBe("compact:off");
  });

  it("writes the answer into the cached row, which is what the tick is painted from", async () => {
    answerSettings = ({ agent_id }) => ({ agent_id, max_context_tokens: 300000, compact_at_tokens: 300000 });
    await mountWorkspaceRail();

    await choose("compact:150000");

    // The bridge answered 300k for a 150k ask: the tick follows the cache the
    // answer was written to, not the row that was pressed.
    expect(await cachedAgent()).toMatchObject({ id: "wa-1", max_context_tokens: 300000, compact_at_tokens: 300000 });
    expect(markedRow().dataset.action).toBe("compact:300000");
  });

  it.each(["answer", "refusal"])("ignores a late compaction %s after the device retires", async (outcome) => {
    let release;
    answerSettings = ({ agent_id, max_context_tokens }) => new Promise((resolve, reject) => {
      release = () => outcome === "refusal"
        ? reject(new Error("retired settings call"))
        : resolve({ agent_id, max_context_tokens, compact_at_tokens: max_context_tokens });
    });
    await mountWorkspaceRail();
    const address = { deviceId: DEVICE_ID, entityId: WORKSPACE_OWNER, kind: "row", sub: "" };
    const before = await readCached(address);

    await choose("compact:off");
    await vi.waitFor(() => expect(settingsAsked).toHaveLength(1));
    const repository = contextFor(DEVICE_ID).chatRepository;
    retireDeviceContext(DEVICE_ID);
    expect(repository.active).toBe(false);
    expect(panel()).not.toBeNull(); // retired, but not yet disposed by the shell
    release();
    await flush();

    expect(await readCached(address)).toEqual(before);
    expect(errorNotices()).toEqual([]);
  });

  // The bridge's sequence at its worst: the answer, then a row it read before
  // the change, then the row its note of the change flushes. The tick follows
  // the cache through all three; nothing writes the answer back.
  it("follows the pushes that come after the answer, stale one included", async () => {
    await mountWorkspaceRail();
    await choose("compact:off");

    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();
    expect(markedRow().dataset.action).toBe("compact:default");

    digestCompaction = { max_context_tokens: 0, compact_at_tokens: 0 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();
    expect(markedRow().dataset.action).toBe("compact:off");
  });

  it("shows a change made elsewhere after the answer, with no echo of the answer first", async () => {
    await mountWorkspaceRail();
    await choose("compact:off");

    digestCompaction = { max_context_tokens: 150000, compact_at_tokens: 150000 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();

    expect(await cachedAgent()).toMatchObject({ max_context_tokens: 150000 });
    expect(markedRow().dataset.action).toBe("compact:150000");
  });

  it("leaves a row a push wrote while the verb was in flight to that push", async () => {
    answerSettings = async ({ agent_id, max_context_tokens }) => {
      digestCompaction = { ...digestCompaction, last_context_tokens: 130000 };
      await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
      return { agent_id, max_context_tokens, compact_at_tokens: 0 };
    };
    await mountWorkspaceRail();

    await choose("compact:off");

    expect(await cachedAgent()).toMatchObject({ max_context_tokens: null, last_context_tokens: 130000 });
    expect(markedRow().dataset.action).toBe("compact:default");
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

  it("says an unsupported compaction command plainly and keeps the cached choice", async () => {
    answerSettings = () => {
      throw Object.assign(new Error("unknown method: conversation.settings"), { code: "unknown_method" });
    };
    await mountWorkspaceRail([]);

    await choose("compact:150000");

    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: 150000 }]);
    expect(errorNotices()).toEqual(["This device does not support changing when this chat compacts.×"]);
    expect((await cachedAgent()).max_context_tokens).toBe(null);
    expect(markedRow().dataset.action).toBe("compact:default");
  });
});
