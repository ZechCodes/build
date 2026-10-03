// @vitest-environment jsdom
// The detail level on the conversation's own menu.
//
// One panel reads two kinds of conversation: a workspace's, where the activity
// IS the news, and a project agent's, which is correspondence. The ⋮ on the
// head is where the reader says which they are reading, and the choice is
// remembered for that conversation alone.
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
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {}, notifySuccess: () => {} }));
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
const { uiAddress } = await import("../src/core/localUiState.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { wipeCache, writeCached, readCached } = await import("../src/core/localCache.js");
const { wipeUiRecords, writeUiRecord, readUiRecord } = await import("../src/core/localUiStore.js");
const { writeRailBoard, writeRailWorkItem } = await import("./railCacheFixture.js");

const DEVICE_ID = "device-1";
const PROJECT_ID = "proj-1";
const WORKSPACE_ID = "ws-1";
const PROJECT_OWNER = "run-project";

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{ id: "claude_adk", label: "Claude Code", models: [{ id: "saved-model", label: "Saved model", supports_effort: true }], efforts: ["low", "high"] }],
};

const CONVERSATION = [{ type: "message", data: { id: "old-message", sequence: 1, role: "agent", body: "the retry is fixed" } }];
let conversation = CONVERSATION;

let currentProvider = "claude_adk";
let resetWait = null;
let modelsWait = null;
const agent = (id, over = {}) => ({
  id,
  ordinal: 1,
  topic: "Fix the retry",
  conversation_id: `conversation-${id}`,
  provider: currentProvider,
  state: "live",
  unread_count: 0,
  working: false,
  thread_id: "old-thread",
  model: "saved-model", effort: "high", max_context_tokens: 150000,
  ...over,
});

const workspacePayload = () => ({
  kind: "branch",
  workspace_id: WORKSPACE_ID,
  project_id: PROJECT_ID,
  entity_id: "run-workspace",
  agents: [agent("wa-1")],
  directories: [],
  thread: { thread_id: "old-thread", items: conversation, sessions: [] },
});

const projectPayload = () => ({
  kind: "branch",
  entity_id: PROJECT_OWNER,
  run_id: PROJECT_OWNER,
  project_id: PROJECT_ID,
  agents: [agent("pa-1")],
  thread: { thread_id: "old-thread", items: conversation, sessions: [] },
});

/** The rail reads the disk and nothing else, so the board and both
 *  conversations go down the way the sync layer would leave them. */
const writeBoard = async () => {
  await writeRailBoard({
    projects: [{ project_id: PROJECT_ID, name: "build", entity_id: PROJECT_OWNER, run_id: PROJECT_OWNER }],
    workspaces: [{
      id: WORKSPACE_ID, project_id: PROJECT_ID, name: "login", status: "ready", entity_id: "run-workspace",
    }],
  }, { deviceId: DEVICE_ID });
  await writeRailWorkItem(workspacePayload(), { deviceId: DEVICE_ID });
  await writeRailWorkItem(projectPayload(), { deviceId: DEVICE_ID });
};

/// The rail settles over the disk — a row read, the conversation beside it,
/// then the paint — and every one of those is a turn.
const flush = async () => {
  for (let count = 0; count < 16; count += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const host = () => document.querySelector("#agent-rail");
const panel = () => host().querySelector("#rail-panel");
const menuCaret = () => panel().querySelector(".rail-surface-menu .caret");
const menuItems = () => [...panel().querySelectorAll(".rail-surface-menu .mi")];
const menuItem = (action) => panel().querySelector(`.rail-surface-menu .mi[data-action="${action}"]`);
const markedLevel = () => menuItems().find((item) => item.getAttribute("aria-checked") === "true")?.dataset.action;

const timeline = () => panel().querySelector(".thread-items");
const rowKinds = () => ({
  user: timeline().querySelectorAll(".thread-message.user").length,
  agent: timeline().querySelectorAll(".thread-message.agent").length,
  arrived: timeline().querySelectorAll(".thread-message.from-agent").length,
  sent: timeline().querySelectorAll(".thread-sent").length,
  activity: timeline().querySelectorAll(".thread-activity-group, .thread-activity").length,
});

let rail;
let calls;
let failReset;

const mountWorkspaceRail = async () => {
  await writeBoard();
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

const mountProjectRail = async () => {
  await writeBoard();
  rail = mountAgentRail(host(), {
    kind: "project",
    deviceId: DEVICE_ID,
    projectId: PROJECT_ID,
    entityId: PROJECT_OWNER,
    cacheScope: contextFor(DEVICE_ID).cacheScope,
    chatRepository: contextFor(DEVICE_ID).chatRepository,
  });
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  await wipeCache();
  await wipeUiRecords();
  conversation = CONVERSATION;
  calls = [];
  failReset = false;
  currentProvider = "claude_adk";
  resetWait = null;
  modelsWait = null;
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  adoptDeviceSession({
    deviceId: DEVICE_ID,
    call: async (method, params) => {
      calls.push({ method, params });
      if (method === "conversation.reset") {
        if (failReset) throw new Error("Could not stop the agent");
        if (resetWait) await resetWait;
        return { ...params, agent_id: params.agent_id, entity_id: params.entity_id, thread_id: "fresh-thread", previous_thread_id: "old-thread",
          agent: agent(params.agent_id, { ...params, id: params.agent_id, thread_id: "fresh-thread", thread_generation_revision: 1, choice_revision: 2, working: false }),
          thread: { thread_id: "fresh-thread", items: [], sessions: [] } };
      }
      if (method === "models.list") { if (modelsWait) await modelsWait; return CATALOG; }
      if (method === "workspace.get") return workspacePayload();
      if (method === "run.get") return projectPayload();
      if (method === "project.list") {
        return { projects: [{ project_id: PROJECT_ID, name: "build", entity_id: PROJECT_OWNER, run_id: PROJECT_OWNER }] };
      }
      return {};
    },
  });
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
});

afterEach(() => {
  rail?.dispose();
  rail = null;
  resetApplication();
  resetChangeEvents();
  vi.useRealTimers();
});

describe("clear conversation on the shared rail", () => {
  it("offers Clear conversation last only when cached support permits it", async () => {
    await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
    await mountWorkspaceRail();
    expect(menuItems().at(-1)?.textContent).toContain("Clear conversation");
    expect(menuItems().at(-1)?.classList.contains("danger")).toBe(true);
  });
  it("keeps an older bridge's menu unchanged", async () => {
    await mountWorkspaceRail();
    expect(menuItem("conversation:clear")).toBe(null);
  });
  it("confirmation cancellation preserves the cached transcript", async () => {
    await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
    await mountWorkspaceRail();
    menuCaret().click();
    menuItem("conversation:clear").click();
    await flush();
    expect(document.querySelector("#confirm-scrim").textContent).toContain("cannot be undone");
    document.querySelector("[data-confirm-cancel]").click();
    await flush();
    expect(timeline().textContent).toContain("the retry is fixed");
  });
  it.each(["workspace", "project"])("prefills the chooser in %s and allows cancel without clearing", async (kind) => {
    await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
    await (kind === "project" ? mountProjectRail() : mountWorkspaceRail());
    menuCaret().click(); menuItem("conversation:clear").click();
    await flush(); document.querySelector("[data-confirm-ok]").click(); await flush();
    expect(panel().querySelector(".rail-harness-choice.chosen").dataset.provider).toBe("claude_adk");
    expect(panel().querySelector("[data-clear-start]").textContent).toBe("Clear and start fresh");
    panel().querySelector("[data-clear-cancel]").click(); await flush();
    expect(timeline().textContent).toContain("the retry is fixed");
    expect(calls.filter((call) => call.method === "conversation.reset")).toHaveLength(0);
  });
});

const beginReset = async () => {
  menuCaret().click(); menuItem("conversation:clear").click();
  await flush(); document.querySelector("[data-confirm-ok]").click(); await flush();
};
it.each(["workspace", "project"])("resets %s atomically with old model, effort, compact and detail defaults", async (kind) => {
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await (kind === "project" ? mountProjectRail() : mountWorkspaceRail());
  await beginReset();
  expect(panel().querySelector("[data-clear-compact]").value).toBe("compact:150000");
  const oldLevel = panel().querySelector("[data-clear-detail]").value;
  panel().querySelector("[data-clear-start]").click(); await flush();
  await vi.waitFor(() => expect(panel().querySelector("[data-clear-start]")).toBe(null));
  const resets = calls.filter((call) => call.method === "conversation.reset");
  expect(resets).toHaveLength(1);
  expect(resets[0].params).toMatchObject({ project_id: PROJECT_ID, provider: "claude_adk", model: "saved-model", effort: "high", max_context_tokens: 150000, expected_thread_id: "old-thread" });
  expect(panel().textContent).not.toContain("the retry is fixed");
  expect(panel().querySelector("[data-clear-start]")).toBe(null);
  const owner = kind === "project" ? PROJECT_OWNER : "run-workspace";
  const id = kind === "project" ? "pa-1" : "wa-1";
  expect((await readCached({ deviceId: DEVICE_ID, entityId: owner, kind: "thread", sub: `conversation-${id}` })).value).toMatchObject({ thread_id: "fresh-thread", items: [] });
  expect((await readUiRecord(uiAddress({ deviceId: DEVICE_ID, entityId: owner, view: "thread", kind: "filter", sub: `conversation-${id}` }))).value.level).toBe(oldLevel);
});
it("retains the transcript and picker after a reset refusal", async () => {
  failReset = true;
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await mountWorkspaceRail(); await beginReset();
  panel().querySelector("[data-clear-start]").click(); await flush();
  expect(panel().querySelector("[data-clear-start]").disabled).toBe(false);
  panel().querySelector("[data-clear-cancel]").click(); await flush();
  expect(timeline().textContent).toContain("the retry is fixed");
});

it("chooses another harness before issuing exactly one reset", async () => {
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await mountProjectRail(); await beginReset();
  const alternative = [...panel().querySelectorAll(".rail-harness-choice")].find((button) => button.dataset.provider !== "claude_adk");
  expect(alternative).toBeTruthy();
  const provider = alternative.dataset.provider;
  alternative.click(); await flush();
  expect(panel().querySelector(".rail-harness-choice.chosen").dataset.provider).toBe(provider);
  expect(calls.filter((call) => call.method === "conversation.reset")).toHaveLength(0);
  panel().querySelector("[data-clear-start]").click(); await flush();
  expect(calls.filter((call) => call.method === "conversation.reset")).toHaveLength(1);
  expect(calls.find((call) => call.method === "conversation.reset").params.provider).toBe(provider);
});

it.each(["pi", "claude", "codex"])("retains exact previous %s harness when the device defaults differ", async (provider) => {
  currentProvider = provider;
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await mountWorkspaceRail(); await beginReset();
  expect(panel().querySelector(".rail-harness-choice.chosen").dataset.provider).toBe(provider);
  panel().querySelector("[data-clear-start]").click(); await flush();
  expect(calls.find((call) => call.method === "conversation.reset").params).toMatchObject({ provider, model: "saved-model", effort: "high" });
});

it("addresses an interrupt to the captured transcript generation", async () => {
  await mountWorkspaceRail();
  await writeCached({ deviceId: DEVICE_ID, entityId: "run-workspace", kind: "row", sub: "" }, {
    ...workspacePayload(), agents: [agent("wa-1", { working: true, can_interrupt: true })],
  });
  await flush();
  panel().querySelector("#railsend").click(); await flush();
  expect(calls.find((call) => call.method === "agent.interrupt").params.thread_id).toBe("old-thread");
});

it("clears the device cache when the accepted reset finishes after leaving the rail", async () => {
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await mountWorkspaceRail(); await beginReset();
  let finish;
  resetWait = new Promise((resolve) => { finish = resolve; });
  panel().querySelector("[data-clear-start]").click(); await flush();
  rail.dispose(); rail = null;
  finish(); await flush();
  expect((await readCached({ deviceId: DEVICE_ID, entityId: "run-workspace", kind: "thread", sub: "conversation-wa-1" })).value.items).toEqual([]);
});

it("keeps the old harness choices when the model catalog arrives after opening the picker", async () => {
  let finish;
  modelsWait = new Promise((resolve) => { finish = resolve; });
  currentProvider = "pi";
  await writeCached({ deviceId: DEVICE_ID, entityId: "", kind: "conversation-reset-support", sub: "" }, { supported: true });
  await mountWorkspaceRail(); await beginReset();
  expect(panel().querySelector(".rail-harness-choice.chosen").dataset.provider).toBe("pi");
  finish(); await flush();
  expect(panel().querySelector(".rail-harness-choice.chosen").dataset.provider).toBe("pi");
  panel().querySelector("[data-clear-start]").click(); await flush();
  expect(calls.find((call) => call.method === "conversation.reset").params).toMatchObject({ provider: "pi", model: "saved-model", effort: "high" });
});
