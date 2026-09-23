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
const { writeRailBoard, writeRailWorkItem } = await import("./railCacheFixture.js");

const DEVICE_ID = "device-1";
const PROJECT_ID = "proj-1";
const WORKSPACE_ID = "ws-1";
const PROJECT_OWNER = "run-project";

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }],
};

const SENDER = { id: "agent-9", owner: { kind: "project", id: "proj-9", name: "build" }, topic: "Deploy" };

/// What arrives is usually a report, and a report past five lines is folded to
/// five with a press underneath (thread.js `arrivalRunsLong`). The fixture is
/// long on purpose: an arrival the level SHOWS has to arrive folded.
const REPORT = ["take the retry path next", ...Array.from({ length: 11 }, (_, line) => `line ${line + 1} of the report`)].join("\n");

/** One conversation holding every kind of thing a timeline draws: the reader,
 *  the agent, a tool call, a message that arrived from elsewhere, and one this
 *  agent sent out. */
const CONVERSATION = [
  { type: "message", data: { id: "m1", sequence: 1, role: "user", body: "look at the retry path" } },
  { type: "tool_use", data: { sequence: 2, event: "tool_use", summary: "read src/retry.rs" } },
  { type: "message", data: { id: "m3", sequence: 3, role: "user", body: REPORT, from_agent: SENDER } },
  { type: "message", data: { id: "m4", sequence: 4, role: "agent", body: "the retry path double-posts", sent_to: SENDER } },
  { type: "message", data: { id: "m5", sequence: 5, role: "agent", body: "the retry is fixed" } },
];

/// A project agent that only relays: everything on its conversation is either
/// another agent's report or the mechanics of passing it on. At its default
/// level there is nothing left to draw.
const RELAY_CONVERSATION = [
  { type: "tool_use", data: { sequence: 1, event: "tool_use", summary: "read src/retry.rs" } },
  { type: "message", data: { id: "r2", sequence: 2, role: "user", body: REPORT, from_agent: SENDER } },
];

/// A conversation holding Build's own words: the notice the bridge posts when
/// it comes back from a restart, beside one of everything a level filters.
const NOTICE_CONVERSATION = [
  { type: "message", data: { id: "n1", sequence: 1, role: "user", body: "look at the retry path" } },
  { type: "tool_use", data: { sequence: 2, event: "tool_use", summary: "read src/retry.rs" } },
  { type: "message", data: { id: "n3", sequence: 3, role: "user", body: REPORT, from_agent: SENDER } },
  { type: "message", data: { id: "n4", sequence: 4, role: "user", body: "The Build bridge restarted. Carry on.", from_build: true } },
  { type: "message", data: { id: "n5", sequence: 5, role: "agent", body: "picking it back up" } },
];

/// Which conversation the machine answers with, so a case can hand over one of
/// its own before mounting.
let conversation = CONVERSATION;

const agent = (id, over = {}) => ({
  id,
  ordinal: 1,
  topic: "Fix the retry",
  conversation_id: `conversation-${id}`,
  provider: "claude_adk",
  state: "live",
  unread_count: 0,
  working: false,
  ...over,
});

const workspacePayload = () => ({
  kind: "branch",
  workspace_id: WORKSPACE_ID,
  project_id: PROJECT_ID,
  entity_id: "run-workspace",
  agents: [agent("wa-1")],
  directories: [],
  thread: { items: conversation, sessions: [] },
});

const projectPayload = () => ({
  kind: "branch",
  entity_id: PROJECT_OWNER,
  run_id: PROJECT_OWNER,
  project_id: PROJECT_ID,
  agents: [agent("pa-1")],
  thread: { items: conversation, sessions: [] },
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
const markedLevel = () => menuItems().find((item) => item.classList.contains("on"))?.dataset.action;

const timeline = () => panel().querySelector(".thread-items");
const rowKinds = () => ({
  user: timeline().querySelectorAll(".thread-message.user").length,
  agent: timeline().querySelectorAll(".thread-message.agent").length,
  arrived: timeline().querySelectorAll(".thread-message.from-agent").length,
  sent: timeline().querySelectorAll(".thread-sent").length,
  activity: timeline().querySelectorAll(".thread-activity-group, .thread-activity").length,
});

let rail;

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

/// The project agent above, mounted on a conversation its default level admits
/// nothing of.
const mountRelayRail = async () => {
  conversation = RELAY_CONVERSATION;
  await mountProjectRail();
};

const choose = async (level) => {
  menuCaret().click();
  menuItem(`detail:${level}`).click();
  await vi.waitFor(() => expect(markedLevel()).toBe(`detail:${level}`));
};
const detailAddress = (entityId, conversationId) => uiAddress({
  deviceId: DEVICE_ID, entityId, view: "thread", kind: "filter", sub: conversationId,
});

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  await wipeCache();
  conversation = CONVERSATION;
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  adoptDeviceSession({
    deviceId: DEVICE_ID,
    call: async (method) => {
      if (method === "models.list") return CATALOG;
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

describe("the detail levels on the conversation's menu", () => {
  it("offers three, above whatever surfaces the agent has", async () => {
    await mountWorkspaceRail();

    expect(menuItems().slice(0, 3).map((item) => item.dataset.action)).toEqual([
      "detail:all",
      "detail:messages",
      "detail:agent",
    ]);
    expect(menuItems().slice(0, 3).map((item) => item.querySelector(".mt").textContent)).toEqual([
      "All",
      "All messages",
      "Agent only",
    ]);
  });

  it("marks the level the conversation is being read at", async () => {
    await mountWorkspaceRail();
    expect(markedLevel()).toBe("detail:all");

    await choose("messages");

    expect(markedLevel()).toBe("detail:messages");
  });
});

describe("what each level draws", () => {
  it("draws activity, both sides of the correspondence, and the dialogue at All", async () => {
    await mountWorkspaceRail();

    expect(rowKinds()).toEqual({ user: 1, agent: 1, arrived: 1, sent: 1, activity: 1 });
  });

  it("drops the activity and keeps every message at All messages", async () => {
    await mountWorkspaceRail();

    await choose("messages");

    expect(rowKinds()).toEqual({ user: 1, agent: 1, arrived: 1, sent: 1, activity: 0 });
    expect(timeline().textContent).not.toContain("read src/retry.rs");
    expect(timeline().textContent).toContain("take the retry path next");
  });

  it("keeps only this agent's words and the reader's at Agent only", async () => {
    await mountWorkspaceRail();

    await choose("agent");

    expect(rowKinds()).toEqual({ user: 1, agent: 1, arrived: 0, sent: 0, activity: 0 });
    expect(timeline().textContent).toContain("look at the retry path");
    expect(timeline().textContent).toContain("the retry is fixed");
    expect(timeline().textContent).not.toContain("take the retry path next");
  });

  it("brings everything back on the way up again", async () => {
    await mountWorkspaceRail();
    await choose("agent");

    await choose("all");

    expect(rowKinds()).toEqual({ user: 1, agent: 1, arrived: 1, sent: 1, activity: 1 });
  });

  it("draws a shown arrival folded to five lines, under the conversation it came from", async () => {
    await mountWorkspaceRail();

    await choose("messages");

    const arrival = timeline().querySelector(".thread-message.from-agent");
    expect(arrival.querySelector(".thread-from").textContent.replace(/\s+/g, " ").trim()).toBe("build › Deploy");
    expect(arrival.querySelector(".thread-comment-card").classList.contains("thread-arrival-folded")).toBe(true);
    expect(arrival.querySelector(".thread-arrival-press").getAttribute("aria-expanded")).toBe("false");
    // Folded, not truncated: the whole report rode the filter through.
    expect(arrival.querySelector(".thread-body").textContent).toContain("line 11 of the report");
  });

  it("folds a shown arrival at All too, and the press still opens it", async () => {
    await mountWorkspaceRail();

    const arrival = () => timeline().querySelector(".thread-message.from-agent");
    const card = () => arrival().querySelector(".thread-comment-card");
    expect(card().classList.contains("thread-arrival-folded")).toBe(true);

    arrival().querySelector(".thread-arrival-press").click();

    expect(card().classList.contains("thread-arrival-folded")).toBe(false);
    expect(arrival().querySelector(".thread-arrival-press").getAttribute("aria-expanded")).toBe("true");
  });

  it("keeps an arrival the reader opened open across a level change and back", async () => {
    await mountWorkspaceRail();
    timeline().querySelector(".thread-arrival-press").click();

    await choose("messages");

    const card = timeline().querySelector(".thread-message.from-agent .thread-comment-card");
    expect(card.classList.contains("thread-arrival-folded")).toBe(false);
  });
});

describe("a timeline the level emptied", () => {
  it("says so, rather than claiming nothing was ever said", async () => {
    await mountRelayRail();

    expect(timeline().textContent).toContain("Nothing at this level");
    expect(timeline().textContent).not.toContain("No conversation yet");
  });

  it("gives the conversation back when the level is widened", async () => {
    await mountRelayRail();

    await choose("all");

    expect(timeline().textContent).not.toContain("Nothing at this level");
    expect(rowKinds()).toEqual({ user: 0, agent: 0, arrived: 1, sent: 0, activity: 1 });
  });

  it("still says nothing was said when there is genuinely nothing", async () => {
    conversation = [];

    await mountWorkspaceRail();

    expect(timeline().textContent).toContain("No conversation yet");
  });
});

describe("the level a conversation opens at", () => {
  it("is everything on a workspace's conversation", async () => {
    await mountWorkspaceRail();

    expect(markedLevel()).toBe("detail:all");
    expect(rowKinds().activity).toBe(1);
  });

  it("is the dialogue alone on a project agent's conversation", async () => {
    await mountProjectRail();

    expect(markedLevel()).toBe("detail:agent");
    expect(rowKinds()).toEqual({ user: 1, agent: 1, arrived: 0, sent: 0, activity: 0 });
  });

  it("gives way to what the reader last chose for that conversation", async () => {
    await writeCached(detailAddress(PROJECT_OWNER, "conversation-pa-1"), { level: "all" });

    await mountProjectRail();

    expect(markedLevel()).toBe("detail:all");
    expect(rowKinds().activity).toBe(1);
  });
});

describe("remembering the choice", () => {
  it("writes it under the conversation's own id", async () => {
    await mountWorkspaceRail();

    await choose("agent");

    expect((await readCached(detailAddress("run-workspace", "conversation-wa-1"))).value.level).toBe("agent");
  });

  it("holds through a remount", async () => {
    await mountWorkspaceRail();
    await choose("messages");
    rail.dispose();
    resetAgentRailMemory();
    document.body.innerHTML = '<div id="agent-rail"></div>';

    await mountWorkspaceRail();

    await vi.waitFor(() => expect(markedLevel()).toBe("detail:messages"));
    expect(rowKinds().activity).toBe(0);
  });
});

describe("the menu's placement on the conversation head", () => {
  // Each case sets the viewport it is about. Put it back afterwards: the rail
  // does not stand its panel up at phone width, so a leaked 390 would break
  // whatever ran next for a reason nowhere near its own subject.
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
  });

  /// The head is a glass bar at the TOP of the panel: `backdrop-filter` makes
  /// `position:fixed` resolve from the head rather than the viewport, and a
  /// menu that preferred to open upward from there ran off the top of the
  /// screen. Both the browser's geometry and the phone width are modelled.
  const openAgainstHead = ({ innerWidth, innerHeight, buttonBox, menuHeight = 140, menuWidth = 220 }) => {
    Object.defineProperty(window, "innerWidth", { value: innerWidth, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: innerHeight, configurable: true });
    const caret = menuCaret();
    const split = caret.closest(".splitbtn");
    const menu = panel().querySelector(".rail-surface-menu .splitmenu");
    panel().style.overflowY = "hidden";
    split.getBoundingClientRect = () => buttonBox;
    const headOffset = { left: buttonBox.left - 40, top: buttonBox.top - 10 };
    Object.defineProperties(menu, {
      offsetWidth: { configurable: true, value: menuWidth },
      offsetHeight: { configurable: true, value: menuHeight },
    });
    menu.getBoundingClientRect = () => {
      const topInset = Number.parseFloat(menu.style.top);
      const bottomInset = Number.parseFloat(menu.style.bottom);
      const top = Number.isFinite(topInset) ? headOffset.top + topInset : headOffset.top - bottomInset;
      return {
        left: headOffset.left + (Number.parseFloat(menu.style.left) || 0),
        right: headOffset.left + (Number.parseFloat(menu.style.left) || 0) + menuWidth,
        top,
        bottom: top + menuHeight,
        width: menuWidth,
        height: menuHeight,
      };
    };
    caret.click();
    return menu;
  };

  it("falls downward from a head at the top of a desktop viewport", async () => {
    await mountWorkspaceRail();

    const menu = openAgainstHead({
      innerWidth: 1440,
      innerHeight: 900,
      buttonBox: { left: 1180, right: 1212, top: 64, bottom: 92, width: 32, height: 28 },
    });

    expect(menu.hidden).toBe(false);
    expect(menu.style.bottom).toBe("auto");
    const placed = menu.getBoundingClientRect();
    expect(placed.top).toBeGreaterThanOrEqual(0);
    expect(placed.bottom).toBeLessThanOrEqual(900);
  });

  it("stays wholly on screen under 760px, where the panel is the page", async () => {
    await mountWorkspaceRail();

    const menu = openAgainstHead({
      innerWidth: 390,
      innerHeight: 720,
      buttonBox: { left: 340, right: 372, top: 56, bottom: 84, width: 32, height: 28 },
    });

    const placed = menu.getBoundingClientRect();
    expect(placed.top).toBeGreaterThanOrEqual(0);
    expect(placed.bottom).toBeLessThanOrEqual(720);
    expect(placed.left).toBeGreaterThanOrEqual(0);
    expect(placed.right).toBeLessThanOrEqual(390);
  });
});

/// The bridge's restart notice is an instruction to the agent, so no level may
/// hide it — and "Agent only" is the level a project agent opens at, which is
/// exactly where these land.
describe("a notice Build wrote, at every level", () => {
  // The placement cases above leave the viewport at phone width, where the
  // rail does not stand its panel up. These are about the timeline, so they
  // ask for a window the panel opens in.
  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
  });

  // #42: a notification is one left-aligned line, not the bubble it wore.
  const notice = () => timeline().querySelector(".thread-message.thread-notice");

  const mountWithNotice = async () => {
    conversation = NOTICE_CONVERSATION;
    await mountWorkspaceRail();
  };

  it("is drawn as one quiet line, with no avatar and no bubble, at All", async () => {
    await mountWithNotice();

    expect(notice()).not.toBeNull();
    expect(notice().querySelector(".thread-issue-notice")).not.toBeNull();
    expect(notice().querySelector(".thread-avatar")).toBeNull();
    expect(notice().querySelector(".thread-comment-card")).toBeNull();
    expect(notice().textContent).toContain("The Build bridge restarted");
  });

  it("survives All messages, where the activity goes", async () => {
    await mountWithNotice();

    await choose("messages");

    expect(notice()).not.toBeNull();
    expect(rowKinds().activity).toBe(0);
  });

  it("survives Agent only, where the correspondence goes", async () => {
    await mountWithNotice();

    await choose("agent");

    expect(notice()).not.toBeNull();
    expect(notice().querySelector(".thread-issue-notice")).not.toBeNull();
    // The arrival beside it is gone, which is what makes this a real test.
    expect(rowKinds()).toMatchObject({ arrived: 0, activity: 0 });
  });

  it("is never drawn as an arrival, at any level", async () => {
    await mountWithNotice();

    for (const level of ["all", "messages", "agent"]) {
      await choose(level);
      expect(notice().classList.contains("from-agent")).toBe(false);
      expect(notice().querySelector(".thread-arrival-press")).toBeNull();
      expect(notice().querySelector(".thread-arrival-folded")).toBeNull();
    }
  });
});
