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
const slider = () => panel().querySelector('.rail-surface-menu [role="slider"]');
const compactionStops = () => JSON.parse(slider().closest(".menu-slider").dataset.options);
const selectedWord = () => slider().getAttribute("aria-valuetext");
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
  if (menuCaret().getAttribute("aria-expanded") !== "true") menuCaret().click();
  const control = slider();
  control.value = String(compactionStops().findIndex((option) => option.id === optionId));
  control.dispatchEvent(new Event("input", { bubbles: true }));
  control.dispatchEvent(new Event("change", { bubbles: true }));
  control.focus();
  control.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
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
  it.each(["Enter", "pointer release", "menu blur"])("keeps the chosen stop while %s's reply is held and never resends it on close", async (gesture) => {
    let release;
    digestCompaction = { max_context_tokens: 150000, compact_at_tokens: 150000 };
    answerSettings = (params) => new Promise((resolve) => {
      release = () => resolve({ ...params, compact_at_tokens: params.max_context_tokens });
    });
    await mountWorkspaceRail();
    menuCaret().click();
    const control = slider();
    control.focus();
    const pointer = (target, type) => target.dispatchEvent(Object.assign(new Event(type, { bubbles: true }), { pointerId: 1 }));
    if (gesture === "pointer release") pointer(control, "pointerdown");
    control.value = "3";
    control.dispatchEvent(new Event("input", { bubbles: true }));
    if (gesture === "pointer release") pointer(control, "pointerup");
    else if (gesture === "menu blur") panel().querySelector('.mi[data-action="detail:agent"]').focus();
    else control.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flush();
    expect(settingsAsked).toHaveLength(1);
    expect(selectedWord()).toBe("300k");
    expect(panel().querySelector('[data-group="compact"] .mt').textContent).toBe("300k");
    expect(slider().value).toBe("3");
    expect((await cachedAgent()).max_context_tokens).toBe(150000);
    expect(menuCaret().getAttribute("aria-expanded")).toBe("true");
    control.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    pointer(document.body, "pointerdown");
    control.blur();
    await flush();
    expect(settingsAsked).toHaveLength(1);
    release();
    await flush();
    expect((await cachedAgent()).max_context_tokens).toBe(300000);
    expect(selectedWord()).toBe("300k");
    expect(menuCaret().getAttribute("aria-expanded")).toBe("false");
  });

  it("restores the cached stop only after a held save fails and allows a retry", async () => {
    let refuse;
    digestCompaction = { max_context_tokens: 150000, compact_at_tokens: 150000 };
    answerSettings = () => new Promise((_resolve, reject) => {
      refuse = () => reject(new Error("settings refused"));
    });
    await mountWorkspaceRail();
    await choose("compact:300000");
    expect(selectedWord()).toBe("300k");
    expect(panel().querySelector('[data-group="compact"] .mt').textContent).toBe("300k");
    refuse();
    await flush();
    expect(selectedWord()).toBe("150k");
    expect(panel().querySelector('[data-group="compact"] .mt').textContent).toBe("150k");
    expect(document.activeElement).toBe(slider());
    expect(menuCaret().getAttribute("aria-expanded")).toBe("true");
    expect((await cachedAgent()).max_context_tokens).toBe(150000);
    answerSettings = (params) => ({ ...params, compact_at_tokens: params.max_context_tokens });
    await choose("compact:300000");
    expect(settingsAsked).toHaveLength(2);
    expect(selectedWord()).toBe("300k");
    expect((await cachedAgent()).max_context_tokens).toBe(300000);
  });

  it("shows the device default the digest names, with its threshold", async () => {
    await mountWorkspaceRail();

    expect(compactionStops().map((option) => option.id)).toEqual([
      "compact:default",
      "compact:150000",
      "compact:200000",
      "compact:300000",
      "compact:off",
    ]);
    expect(selectedWord()).toBe("Default (200k)");
    // The group names the setting; the slider offers only its discrete stops.
    expect(slider().closest('[role="group"]').getAttribute("aria-label")).toBe("Compact at");
    expect(slider().getAttribute("role")).toBe("slider");
    expect([slider().min, slider().max, slider().step]).toEqual(["0", "4", "1"]);
    expect(panel().querySelectorAll('[data-group="compact"] .mt')).toHaveLength(1);
  });

  it("shows the conversation's own limit when the digest carries one", async () => {
    digestCompaction = { max_context_tokens: 300000, compact_at_tokens: 300000 };
    await mountWorkspaceRail();

    expect(slider().dataset.action).toBe("compact:300000");
  });

  it("shows a limit set elsewhere on a stop of its own, with its value", async () => {
    digestCompaction = { max_context_tokens: 250000, compact_at_tokens: 250000 };
    await mountWorkspaceRail();

    expect(compactionStops().map((option) => option.id)).toEqual([
      "compact:default",
      "compact:150000",
      "compact:200000",
      "compact:300000",
      "compact:250000",
      "compact:off",
    ]);
    expect([slider().value, slider().max]).toEqual(["4", "5"]);
    expect(slider().dataset.action).toBe("compact:250000");
    expect(selectedWord()).toBe("Custom (250k)");
  });

  it("sends nothing for the custom stop already standing, and leaves it for an offered size", async () => {
    digestCompaction = { max_context_tokens: 250000, compact_at_tokens: 250000 };
    await mountWorkspaceRail();

    await choose("compact:250000");
    expect(settingsAsked).toEqual([]);
    // A row this menu drew never falls through to opening a surface.
    expect(document.querySelector(".modal-scrim")).toBe(null);

    await choose("compact:150000");
    expect(settingsAsked).toEqual([{ entity_id: WORKSPACE_OWNER, agent_id: "wa-1", max_context_tokens: 150000 }]);
    expect(slider().dataset.action).toBe("compact:150000");
    expect(compactionStops().map((option) => option.id)).not.toContain("compact:250000");
  });

  it("shows the digest's compaction choice before greeting", async () => {
    digestCompaction = { max_context_tokens: 300000, compact_at_tokens: 300000 };
    await mountWorkspaceRail(null);

    expect(slider().dataset.action).toBe("compact:300000");
  });

  it("keeps the compaction stops when a bridge's greeting does not name conversations.settings", async () => {
    await mountWorkspaceRail([]);

    expect(menuCaret()).not.toBe(null);
    expect(compactionStops()).toHaveLength(5);
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
    expect(selectedWord()).toBe("Default (200k)");
  });

  it("marks what the bridge answered", async () => {
    await mountWorkspaceRail();

    await choose("compact:off");

    expect(slider().dataset.action).toBe("compact:off");
  });

  it("writes the answer into the cached row, which is what the tick is painted from", async () => {
    answerSettings = ({ agent_id }) => ({ agent_id, max_context_tokens: 300000, compact_at_tokens: 300000 });
    await mountWorkspaceRail();

    await choose("compact:150000");

    // The bridge answered 300k for a 150k ask: the tick follows the cache the
    // answer was written to, not the row that was pressed.
    expect(await cachedAgent()).toMatchObject({ id: "wa-1", max_context_tokens: 300000, compact_at_tokens: 300000 });
    expect(slider().dataset.action).toBe("compact:300000");
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
    expect(slider().dataset.action).toBe("compact:default");

    digestCompaction = { max_context_tokens: 0, compact_at_tokens: 0 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();
    expect(slider().dataset.action).toBe("compact:off");
  });

  it("shows a change made elsewhere after the answer, with no echo of the answer first", async () => {
    await mountWorkspaceRail();
    await choose("compact:off");

    digestCompaction = { max_context_tokens: 150000, compact_at_tokens: 150000 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();

    expect(await cachedAgent()).toMatchObject({ max_context_tokens: 150000 });
    expect(slider().dataset.action).toBe("compact:150000");
  });

  it("leaves a pushed row untouched while holding the accepted stop until its cache push", async () => {
    answerSettings = async ({ agent_id, max_context_tokens }) => {
      digestCompaction = { ...digestCompaction, last_context_tokens: 130000 };
      await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
      return { agent_id, max_context_tokens, compact_at_tokens: 0 };
    };
    await mountWorkspaceRail();

    await choose("compact:off");

    expect(await cachedAgent()).toMatchObject({ max_context_tokens: null, last_context_tokens: 130000 });
    expect(slider().dataset.action).toBe("compact:off");
    digestCompaction = { ...digestCompaction, max_context_tokens: 0, compact_at_tokens: 0 };
    await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
    await flush();
    expect(await cachedAgent()).toMatchObject({ max_context_tokens: 0, last_context_tokens: 130000 });
    expect(slider().dataset.action).toBe("compact:off");
  });

  it("says a refusal in a sentence and leaves the choice where it was", async () => {
    answerSettings = () => {
      throw new Error("agent wa-1 is not on run-workspace");
    };
    await mountWorkspaceRail();

    await choose("compact:150000");

    expect(errorNotices().some((text) => text.includes("Build could not change when this chat compacts."))).toBe(true);
    expect(slider().dataset.action).toBe("compact:default");
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
    expect(slider().dataset.action).toBe("compact:default");
  });
});
