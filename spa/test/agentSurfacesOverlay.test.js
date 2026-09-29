// @vitest-environment jsdom
import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { surfacesSnapshot } from "./surfacesFixture.js";
import { motionBeat } from "./motionRecorder.js";
import { sessionAnswering } from "./deviceSessionFixture.js";
import { EXITING_ATTRIBUTE } from "../src/core/patchList.js";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// The rail paints from the cache, so this suite gives the modules a database
// before anything imports them.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");
const appCss = readFileSync(resolve("src/styles.css"), "utf8");

vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: async () => [],
  dropFeedDevice: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: async () => {},
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose: () => {} }) }));

const { App } = await import("../src/app.js");
const { adoptBridgeSelection, adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { mountAgentRail, panelHeadHtml, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { openSurfaceOverlay } = await import("../src/core/agentSurfaces.js");
const { AGENT_ENTRY_KIND, SHELL_ENTRY_KIND, TASKS_ENTRY_KIND, WORKFLOW_ENTRY_KIND } = await import("../src/core/agentSurfacesModel.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { writeRailWorkItem } = await import("./railCacheFixture.js");
const { rememberAgentLineageSupport } = await import("../src/core/agentLineageSupport.js");
const { writeTasksRecord } = await import("../src/core/trackerCache.js");
const { columns, task } = await import("./trackerWireFixture.js");

const surfaces = () => surfacesSnapshot({ subagents: [], checklist: [] });

const branchRow = (agentOver = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [
    {
      id: "ag-1",
      ordinal: 1,
      provider: "claude",
      state: "live",
      unread_count: 0,
      working: false,
      surface_session_generation: "session-one",
      surfaces: surfaces(),
      ...agentOver,
    },
  ],
  run: { run_id: "run-3", thread: { items: [], sessions: [] } },
});

let payload = branchRow();
let calls = [];
let rail = null;
/** What the machine this rail is mounted on offers to start work with. */
let catalog = { default_provider: "claude", providers: [] };

// The rail settles over the disk: its row, and the conversation in it —
// every record it opens is a turn.
const flush = async () => {
  for (let i = 0; i < 12; i++) await new Promise((done) => setTimeout(done, 0));
};

const panel = () => document.getElementById("rail-panel");
const menuCaret = () => panel().querySelector(".rail-surface-menu .caret");
const menuItems = () => [...panel().querySelectorAll(".rail-surface-menu .mi")];
/// The menu's SURFACE half. Its other half is the detail levels the
/// conversation is read at (core/conversationDetail.js), which are always
/// there — and so are not what these tests are about.
const surfaceMenuItems = () => menuItems().filter((item) => !item.dataset.action.startsWith("detail:"));
const menuItem = (kind) => panel().querySelector(`.rail-surface-menu .mi[data-action="${kind}"]`);
const openMenuElement = () => panel().querySelector(".rail-surface-menu .splitmenu");
const overlay = () => document.querySelector(".modal-surface");
const overlayRows = () =>
  [...document.querySelectorAll(`.modal-surface .surface-running > .surface-row:not([${EXITING_ATTRIBUTE}])`)];
const pressEscape = () =>
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

const watchDocumentListeners = (watchedType) => {
  const live = new Set();
  const add = document.addEventListener.bind(document);
  const remove = document.removeEventListener.bind(document);
  document.addEventListener = (type, ...rest) => {
    if (type === watchedType) live.add(rest[0]);
    return add(type, ...rest);
  };
  document.removeEventListener = (type, ...rest) => {
    if (type === watchedType) live.delete(rest[0]);
    return remove(type, ...rest);
  };
  return {
    count: () => live.size,
    stop: () => {
      delete document.addEventListener;
      delete document.removeEventListener;
    },
  };
};

const watchArmedTimeouts = () => {
  const armed = new Set();
  const arm = globalThis.setTimeout;
  const clear = globalThis.clearTimeout;
  globalThis.setTimeout = (run, delayMs, ...rest) => {
    const id = arm(run, delayMs, ...rest);
    if (delayMs > 0) armed.add(id);
    return id;
  };
  globalThis.clearTimeout = (id) => {
    armed.delete(id);
    return clear(id);
  };
  return {
    count: () => armed.size,
    stop: () => {
      globalThis.setTimeout = arm;
      globalThis.clearTimeout = clear;
    },
  };
};

const finishedShells = () => {
  const row = branchRow();
  row.agents[0].surfaces.shells = [{ id: "sh1", description: "cargo test", state: "done", exit_code: 0, tail: [] }];
  return row;
};

const mount = async () => {
  await writeRailWorkItem(payload);
  rail = mountAgentRail(document.getElementById("agent-rail"), {
    kind: "branch",
    deviceId: "dev-1",
    projectId: "p1",
    branch: "build/login",
    // A standalone mount brings its own caller: the rail makes its repository
    // over the machine it was handed, not over an ambient one.
    call: (method, params) => bridge.call(method, params),
  });
  await flush();
};

/** The row moved: what a push writes, and what the rail hears. */
const poll = async (next) => {
  payload = next;
  await writeRailWorkItem(next);
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  await wipeCache();
  resetAgentRailMemory();
  notifyError.mockClear();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  calls = [];
  payload = branchRow();
  catalog = { default_provider: "claude", providers: [] };
  bridge.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "models.list") return catalog;
    if (method === "branch.get") return payload;
    if (method === "thread.post") return { posted_sequence: 7 };
    return {};
  });
  // The machine the rail is mounted on, which is the one its harness catalog
  // comes from.
  adoptBridgeSelection(adoptDeviceSession(sessionAnswering(bridge)), { version: "2.0.0" }, null); // landed and greeted
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
  resetDeviceContexts();
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("panelHeadHtml's surface menu", () => {
  it("keeps the title as text beside the selected harness icon", () => {
    const html = panelHeadHtml("My agent", "chat", { provider: "codex_app_server" });
    document.body.innerHTML = html;

    expect(document.querySelector(".rail-who").textContent).toBe("My agent");
    expect(document.querySelector(".rail-harness-icon").dataset.harnessIcon).toBe("codex_app_server");
  });

  it("writes no menu at all when the agent has nothing to show", () => {
    const html = panelHeadHtml("Claude Code", "chat", { menuGroups: [] });
    expect(html).toContain("rail-surface-menu");
    expect(html).not.toContain("splitbtn");
  });

  it("places the pin between Done and the vertical menu without a collapse control", () => {
    const html = panelHeadHtml("Claude Code", "chat", {
      removable: true,
      menuGroups: [{ id: "show", label: "Show", options: [{ id: SHELL_ENTRY_KIND, label: "Shells", description: "1 running" }] }],
    });
    expect(html).toContain(`data-action="${SHELL_ENTRY_KIND}"`);
    expect(html).toContain("Shells");
    expect(html).toContain("1 running");
    expect(html.indexOf("rail-remove")).toBeLessThan(html.indexOf("pinbtn"));
    expect(html.indexOf("pinbtn")).toBeLessThan(html.indexOf("rail-surface-menu"));
    expect(html).toContain(">Done</button>");
    expect(html).toContain("⋮");
    expect(html).not.toContain("rail-collapse");
  });
});

describe("openSurfaceOverlay", () => {
  it("mounts the kind's viewer in a modal and keeps it current", async () => {
    const held = openSurfaceOverlay(SHELL_ENTRY_KIND, { onOpenThreadItem: () => {} });
    held.set(surfaces());

    expect(overlay().querySelector("h3").textContent).toBe("Shells");
    expect(overlayRows()).toHaveLength(1);

    const grown = surfaces();
    grown.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });
    held.set(grown);
    expect(overlayRows()).toHaveLength(2);

    await held.close();
    expect(overlay()).toBe(null);
  });

  it("keeps the empty viewer up when the kind loses everything under the reader", () => {
    const held = openSurfaceOverlay(SHELL_ENTRY_KIND, { onOpenThreadItem: () => {} });
    held.set(surfaces());

    held.set({ workflows: surfaces().workflows });

    expect(overlay()).not.toBe(null);
    expect(overlay().querySelector(".surface-shells")).not.toBe(null);
    expect(overlayRows()).toEqual([]);
    held.close();
  });

  it("paints rows with the width it has, naming models through the label it was handed", () => {
    const held = openSurfaceOverlay(AGENT_ENTRY_KIND, {
      onOpenThreadItem: () => {},
      modelLabel: (modelId) => `Opus 5 · ${modelId}`,
    });
    held.set({
      subagents: [{ id: "a1", label: "reader", state: "running", model: "opus", tokens: 1200, tool_calls: 4 }],
    });

    const row = overlayRows()[0];
    expect(row.querySelector(".surface-row-model").textContent).toBe("Opus 5 · opus");
    expect(row.querySelector(".surface-agent-facts").textContent).toContain("Tokens");
    expect(row.querySelector(".surface-agent-facts").textContent).toContain("1200");
    held.close();
  });

  it("tells its caller once when Escape takes it away", async () => {
    const onClose = vi.fn();
    const held = openSurfaceOverlay(WORKFLOW_ENTRY_KIND, { onOpenThreadItem: () => {}, onClose });
    held.set(surfaces());

    pressEscape();
    await motionBeat();

    expect(overlay()).toBe(null);
    expect(onClose).toHaveBeenCalledTimes(1);
    await held.close();
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe("the conversation header's menu", () => {
  it("offers one option per kind the agent has content in", async () => {
    await mount();
    expect(surfaceMenuItems().map((item) => item.dataset.action)).toEqual([WORKFLOW_ENTRY_KIND, SHELL_ENTRY_KIND]);
    expect(menuItem(SHELL_ENTRY_KIND).textContent).toContain("Shells");
    expect(menuItem(SHELL_ENTRY_KIND).textContent).toContain("1 running");
  });

  it("is one plain icon button carrying the three dots alone", async () => {
    await mount();
    expect(menuCaret().classList.contains("iconbtn")).toBe(true);
    expect(menuCaret().textContent.trim()).toBe("⋮");
    expect(menuCaret().closest(".splitbtn").classList.contains("splitbtn-icon")).toBe(true);
  });

  it("opens from the glass header's containing block and accepts an item click", async () => {
    await mount();
    const caret = menuCaret();
    const menu = openMenuElement();
    const split = caret.closest(".splitbtn");
    panel().style.overflowY = "hidden";
    const headerOffset = { left: 700, top: 12 };
    split.getBoundingClientRect = () => ({ left: 800, right: 832, top: 20, bottom: 48, width: 32, height: 28 });
    Object.defineProperties(menu, {
      offsetWidth: { configurable: true, value: 180 },
      offsetHeight: { configurable: true, value: 96 },
    });
    // backdrop-filter makes the fixed menu resolve from the header rather
    // than the viewport. Model that browser geometry so this test catches a
    // menu that opens successfully but lands beyond the visible panel.
    menu.getBoundingClientRect = () => ({
      left: headerOffset.left + (Number.parseFloat(menu.style.left) || 0),
      top: headerOffset.top + (Number.parseFloat(menu.style.top) || 0),
      width: 180,
      height: 96,
    });

    caret.click();

    expect(menu.hidden).toBe(false);
    expect(menu.getBoundingClientRect()).toMatchObject({ left: 652, top: 54 });
    menuItem(SHELL_ENTRY_KIND).click();
    await flush();
    expect(overlay()).not.toBe(null);
  });

  it("still opens on the detail levels while the agent has no surfaces at all", async () => {
    payload = branchRow({ surfaces: null });
    await mount();
    expect(menuCaret()).not.toBe(null);
    expect(surfaceMenuItems()).toEqual([]);
    expect(menuItems().map((item) => item.dataset.action)).toEqual(["detail:all", "detail:messages", "detail:agent"]);
    // The group of things to open is absent, not empty.
    expect(panel().querySelector('.rail-surface-menu [data-group="show"]')).toBe(null);
  });

  it("heads the surfaces as things to show, apart from the settings under them", async () => {
    await mount();
    const groups = [...panel().querySelectorAll('.rail-surface-menu [role="group"]')];
    expect(groups.map((group) => group.getAttribute("aria-label"))).toEqual(["Show", "Detail"]);
    expect([...groups[0].querySelectorAll(".mi")].map((item) => item.dataset.action)).toEqual([WORKFLOW_ENTRY_KIND, SHELL_ENTRY_KIND]);
  });

  it("leaves an open menu open through a read that lists the same options", async () => {
    await mount();
    menuCaret().click();
    const menu = openMenuElement();

    const moved = branchRow();
    moved.agents[0].surfaces.shells[0].tail = ["running 12 tests", "test parser::folds ... ok"];
    await poll(moved);

    expect(openMenuElement()).toBe(menu);
    expect(menu.hidden).toBe(false);
  });

  it("repaints the options when the agent gains a kind", async () => {
    await mount();
    const gained = branchRow();
    gained.agents[0].surfaces.checklist = [{ id: "c1", subject: "Land the fold", state: "in_progress" }];

    await poll(gained);

    expect(surfaceMenuItems().map((item) => item.dataset.action)).toEqual([
      WORKFLOW_ENTRY_KIND,
      SHELL_ENTRY_KIND,
      "checklist",
    ]);
  });

  it("loses its surface rows when the agent's last surface does, and keeps the levels", async () => {
    await mount();
    expect(surfaceMenuItems()).not.toEqual([]);

    await poll(branchRow({ surfaces: {} }));

    expect(menuCaret()).not.toBe(null);
    expect(surfaceMenuItems()).toEqual([]);
  });
});

describe("the agent's goal and observed checklist", () => {
  const observed = () => ({
    goal: { objective: "Ship the release", state: "active" },
    checklist: [{ id: "one", subject: "Run verification", state: "in_progress" }],
    observations: {
      goal: { support: "supported", freshness: "current", coverage: "complete" },
      checklist: { support: "supported", freshness: "current", coverage: "complete" },
    },
  });

  it("paints a goal-only observation above the composer", async () => {
    payload = branchRow({ surfaces: { goal: { objective: "Ship the release", state: "paused" }, observations: {} } });
    await mount();

    const observation = panel().querySelector(".agent-observation-host");
    expect(observation.hidden).toBe(false);
    expect(observation.textContent).toContain("Goal paused");
    expect(observation.textContent).toContain("Ship the release");
  });

  it("moves the observed checklist into a Tasks activity", async () => {
    payload = branchRow({ surfaces: observed() });
    await mount();

    expect(panel().querySelector(".agent-observation-checklist")).toBe(null);
    const tasks = panel().querySelector('[data-surface-kind="checklist"]');
    expect(tasks.textContent).toContain("Checklist");
    expect(tasks.textContent).toContain("0/1");
  });

  it("shows task observation metadata inside the Tasks viewer", async () => {
    payload = branchRow({ surfaces: observed() });
    await mount();
    panel().querySelector('[data-surface-kind="checklist"]').click();
    await vi.waitFor(() => expect(panel().querySelector(".surface-checklist-context")?.textContent).toContain("Run verification"));
    expect(panel().querySelector(".surface-checklist-context").textContent).toContain("Run verification");
    expect(panel().querySelector(".surface-checklist-context").textContent).toContain("0/1");
  });

  it("keeps partial, stale, and prior-turn task context in the viewer", async () => {
    const surfaces = observed();
    surfaces.checklist[0].state = "completed";
    surfaces.checklist_provenance = { carried_from_prior_turn: true };
    surfaces.observations.checklist = {
      support: "supported",
      freshness: "stale",
      coverage: "partial",
      omitted_count: 3,
    };
    payload = branchRow({ surfaces });
    await mount();

    panel().querySelector('[data-surface-kind="checklist"]').click();
    await vi.waitFor(() => expect(panel().querySelector(".surface-checklist-context")?.textContent).toContain("1 known completed"));
    const context = panel().querySelector(".surface-checklist-context");
    expect(context.textContent).toContain("1 known completed · 3 omitted");
    expect(context.textContent).toContain("Last known");
    expect(context.textContent).toContain("Prior turn");
    expect(context.textContent).toContain("Partial");
    expect(context.classList.contains("is-stale")).toBe(true);
  });
});

describe("the model a surface row names", () => {
  it("says what the account's catalog calls it, and the raw id when it knows none", async () => {
    catalog = {
      default_provider: "claude",
      providers: [
        { id: "claude", label: "Claude Code", models: [{ id: "claude-opus-5[1m]", label: "Opus 5 · 1m" }], efforts: [] },
      ],
    };
    const named = surfaces();
    named.workflows[0].phases[0].agents[0].model = "claude-opus-5[1m]";
    payload = branchRow({ surfaces: named });
    await mount();

    menuCaret().click();
    menuItem(WORKFLOW_ENTRY_KIND).click();
    await flush();

    expect(document.querySelector(".modal-surface .surface-row-model").textContent).toBe("Opus 5 · 1m");
  });

  it("falls back to the id itself while the catalog holds no such model", async () => {
    const named = surfaces();
    named.workflows[0].phases[0].agents[0].model = "some-unlisted-model";
    payload = branchRow({ surfaces: named });
    await mount();

    menuCaret().click();
    menuItem(WORKFLOW_ENTRY_KIND).click();
    await flush();

    expect(document.querySelector(".modal-surface .surface-row-model").textContent).toBe("some-unlisted-model");
  });
});

describe("the surface a menu option opens", () => {
  const openShells = async () => {
    await mount();
    menuCaret().click();
    menuItem(SHELL_ENTRY_KIND).click();
    await vi.waitFor(() => expect(overlayRows().map((row) => row.dataset.key)).toEqual(["sh1"]));
  };

  it("lays over the chat panel alone, not the whole page", async () => {
    await openShells();
    const scrim = panel().querySelector(".modal-scrim");
    expect(scrim).not.toBe(null);
    expect(scrim.classList.contains("modal-scrim-local")).toBe(true);
    expect(scrim.querySelector(".modal-surface")).toBe(overlay());
    expect(document.body.querySelector(":scope > .modal-scrim")).toBe(null);
  });

  it("shows the kind's rows over the panel", async () => {
    await openShells();
    expect(overlay()).not.toBe(null);
    expect(overlayRows().map((row) => row.dataset.key)).toEqual(["sh1"]);
    expect(overlay().textContent).toContain("cargo test");
  });

  it("takes every later read while it is open", async () => {
    await openShells();
    const grown = branchRow();
    grown.agents[0].surfaces.shells.push({ id: "sh2", description: "cargo clippy", state: "running", tail: [] });

    await poll(grown);

    expect(overlayRows().map((row) => row.dataset.key)).toEqual(["sh1", "sh2"]);
  });

  it("hangs no menu off an overlay row", async () => {
    await openShells();

    expect(overlay().querySelectorAll(".splitbtn")).toHaveLength(0);
    expect(calls.filter((call) => call.method === "thread.post")).toEqual([]);
  });

  it("closes on Escape and leaves nothing of itself behind", async () => {
    await openShells();
    pressEscape();
    await flush();

    expect(overlay()).toBe(null);
    expect(document.querySelector(".modal-scrim")).toBe(null);
  });

  it("takes its viewer with it, so a later read paints no rows off screen", async () => {
    await openShells();
    pressEscape();
    await flush();

    await poll(branchRow());

    expect(document.querySelectorAll(".modal-surface")).toHaveLength(0);
  });

  it("closes when the panel it stands over changes agents", async () => {
    await openShells();
    const another = branchRow();
    another.agents.push({ ...another.agents[0], id: "ag-2", ordinal: 2, surfaces: null });
    await poll(another);
    [...document.querySelectorAll(".rail-bubble")][1].click();
    await flush();

    expect(overlay()).toBe(null);
  });
});

// #225: the Build agents (#216) and the tasks (#34) come from the project's
// cached rows and task list, not the digest — the ⋮ menu's overlay has to
// draw them just as the pill's panel does.
describe("a menu option over what the digest does not carry", () => {
  it("opens Agents on the Build agents this agent made", async () => {
    const row = branchRow();
    row.agents.push({ ...row.agents[0], id: "ag-2", ordinal: 2, name: "Login fixer", created_by: "ag-1", surfaces: null });
    payload = row;
    await mount();
    await rememberAgentLineageSupport("dev-1", { agents: { createdBy: true } });
    await vi.waitFor(() => expect(menuItem(AGENT_ENTRY_KIND)).not.toBe(null));

    menuCaret().click();
    menuItem(AGENT_ENTRY_KIND).click();

    await vi.waitFor(() =>
      expect([...overlay().querySelectorAll(".surface-build-agents > .surface-build-agent")].map((one) => one.textContent))
        .toEqual([expect.stringContaining("Login fixer")]));
  });

  it("opens Tasks on the tasks this agent carries", async () => {
    await writeTasksRecord("dev-1", "p1", {
      tasks: [task({ number: 7, id: "i7", title: "Fix login", status: "in_progress", assignee: { kind: "agent", agent_id: "ag-1" } })],
      columns: columns(),
    });
    await mount();
    await vi.waitFor(() => expect(menuItem(TASKS_ENTRY_KIND)).not.toBe(null));

    menuCaret().click();
    menuItem(TASKS_ENTRY_KIND).click();

    await vi.waitFor(() => expect(overlay().textContent).toContain("#7 Fix login"));
  });

  it("takes a pushed task list while Tasks is open", async () => {
    const mine = (number, title) =>
      task({ number, id: `i${number}`, title, status: "in_progress", assignee: { kind: "agent", agent_id: "ag-1" } });
    await writeTasksRecord("dev-1", "p1", { tasks: [mine(7, "Fix login")], columns: columns() });
    await mount();
    await vi.waitFor(() => expect(menuItem(TASKS_ENTRY_KIND)).not.toBe(null));
    menuCaret().click();
    menuItem(TASKS_ENTRY_KIND).click();
    await vi.waitFor(() => expect(overlay().textContent).toContain("#7 Fix login"));

    await writeTasksRecord("dev-1", "p1", { tasks: [mine(7, "Fix login"), mine(8, "Fix logout")], columns: columns() });

    await vi.waitFor(() => expect(overlay().textContent).toContain("#8 Fix logout"));
  });
});

describe("what the ⋯ leaves on the document", () => {
  it("takes its outside-press watch with it when the rail is disposed", async () => {
    await mount();
    const watching = watchDocumentListeners("pointerdown");
    menuCaret().click();
    expect(watching.count()).toBe(1);

    rail.dispose();
    rail = null;

    expect(watching.count()).toBe(0);
    watching.stop();
  });

  it("takes it with it when a read changes what the menu offers", async () => {
    await mount();
    const watching = watchDocumentListeners("pointerdown");
    menuCaret().click();

    const gained = branchRow();
    gained.agents[0].surfaces.checklist = [{ id: "c1", subject: "Land the fold", state: "in_progress" }];
    await poll(gained);

    expect(watching.count()).toBe(0);
    watching.stop();
  });
});

describe("collapsing the panel a surface stands over", () => {
  it("closes the overlay while retaining panel state until disposal", async () => {
    await mount();
    menuCaret().click();
    menuItem(SHELL_ENTRY_KIND).click();
    expect(overlay()).not.toBe(null);

    const timers = watchArmedTimeouts();
    await poll(finishedShells());
    expect(timers.count()).toBe(1);

    const standingPanel = document.getElementById("rail-panel");
    document.querySelector(".rail-bubble").click();
    await flush();

    expect(document.getElementById("rail-panel")).toBe(standingPanel);
    expect(standingPanel.getAttribute("aria-hidden")).toBe("true");
    expect(standingPanel.hasAttribute("inert")).toBe(true);
    expect(overlay()).toBe(null);
    expect(document.querySelector(".modal-scrim")).toBe(null);
    expect(timers.count()).toBeGreaterThanOrEqual(1);
    rail.dispose();
    rail = null;
    expect(timers.count()).toBe(0);
    timers.stop();
  });
});

describe("the overlay's own height", () => {
  it("is a modal, not the rail's capped viewer region", async () => {
    await mount();
    menuCaret().click();
    menuItem(SHELL_ENTRY_KIND).click();

    expect(overlay().closest(".rail-surfaces-viewer")).toBe(null);
    expect(shellCss).toMatch(/\.rail-surfaces-viewer\s*\{[^}]*max-height:min\(46vh, 420px\)/);
    expect(shellCss).toMatch(/\.surface-popover-body\s*\{[^}]*overflow-y:auto/);
    expect(shellCss).not.toContain(".modal-surface");
    expect(appCss).toMatch(/\.modal\.modal-surface\s*\{[^}]*max-height/);
    expect(appCss).toMatch(/\.modal-surface\s+\.surface-overlay-body\s*\{[^}]*overflow-y:auto/);
  });
});
