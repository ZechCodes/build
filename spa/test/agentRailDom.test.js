// @vitest-environment jsdom
// The agent rail's wiring: the strip that is always there, the panel that
// expands beside it, the two faces of an agent, and the first message — which
// on a checkout Build owns nothing in is what brings the agent into being.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";
import { sessionAnswering } from "./deviceSessionFixture.js";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

// The device's conversations, as the registry holds them for this one machine:
// the rail is handed one and writes every draft and message through it.
let chatRepository = null;

// The conversation cache writes through IndexedDB; give the module a fake one
// before anything imports it.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");

const refreshFeed = vi.fn(async () => []);
// A Set, matching the real module (core/taskFeed.js) — more than one
// subscriber at once is the normal case there, not an edge case to special-
// case away.
const feedSubscribers = new Set();
let feedSnapshot = { items: [], projects: [] };
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    feedSubscribers.add(fn);
    fn(feedSnapshot);
    return () => feedSubscribers.delete(fn);
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  dropFeedDevice: () => {},
}));
const markSeen = vi.fn(async () => {});
vi.mock("../src/core/inboxView.js", () => ({
  markSeen: (...args) => markSeen(...args),
  noteSelfAction: async () => {},
  mountInboxList: () => {},
  inboxListRouteChanged: () => {},
}));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notifySuccess: () => {} }));
const mountAgentTab = vi.fn(() => ({ dispose: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: (...args) => mountAgentTab(...args) }));

// The painter behind a bubble's face, standing in for the real one: jsdom has
// no 2D context to draw into, and what the rail owes the painter is a lifecycle
// — one per bubble, told what it should be doing, let go when its agent leaves.
const painters = [];
vi.mock("../src/core/agentCanvas.js", () => ({
  createPatternRenderer: (options) => {
    const painter = {
      options,
      working: false,
      ink: null,
      dimmed: false,
      destroyed: false,
      setWorking: (next) => {
        painter.working = !!next;
      },
      setInk: (next) => {
        painter.ink = next;
      },
      setDimmed: (next) => {
        painter.dimmed = !!next;
      },
      destroy: () => {
        painter.destroyed = true;
      },
      isWorking: () => painter.working,
    };
    painters.push(painter);
    return painter;
  },
  animatingRendererCount: () => painters.filter((painter) => painter.working && !painter.destroyed).length,
}));

const { App } = await import("../src/app.js");
const { scopeFor } = await import("../src/core/cacheScope.js");
const { adoptDeviceSession, contextFor, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { createChatRepository } = await import("../src/core/chatRepository.js");
const { evictEntity, readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { motionSettled } = await import("../src/core/motion.js");
const { insertRecord, resetOptimistic, runOptimistic } = await import("../src/core/optimistic.js");
const { SURFACE_PILL_GRACE_MS, writeOpenSurface } = await import("../src/core/agentSurfacesModel.js");
const { surfacesCacheAddress, surfacesRecord } = await import("../src/core/surfacesCache.js");
const { createAgentSelection } = await import("../src/core/agentSelection.js");
const { createAdoptingCall } = await import("../src/core/adoption.js");
const { FIRST_PAGE_ITEMS } = await import("../src/core/thread.js");
const { resetUsageLimits, setUsageLimits } = await import("../src/core/usageLimits.js");
const { ACTIVITY_RECORD_KIND } = await import("../src/core/activityRuns.js");
const { pushRailThreadItems, writeRailBoard, writeRailThread, writeRailWorkItem } = await import("./railCacheFixture.js");

/** The topic each fixture agent named its work with. The head and the bubbles
 *  say the topic now, so it is the topic — not a harness and an ordinal — that
 *  tells these cases which agent the panel is open on. An agent left out of the
 *  table has named nothing yet, and says so. */
const TOPICS = { "ag-1": "Fix login redirect", "ag-2": "Polish the rail" };

const agent = (over = {}) => {
  const row = {
    id: "ag-1", ordinal: 1, provider: "claude_adk", state: "live",
    unread_count: 0, unread_reason: null, working: false,
    surface_session_generation: "surface-session-1", ...over,
  };
  return { topic: TOPICS[row.id] || "", ...row };
};

const branchRow = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  branch: "build/login",
  run_id: "run-3",
  worktree_id: "wt-3",
  agents: [agent()],
  run: { run_id: "run-3", thread: { items: [], sessions: [] } },
  ...over,
});

let payload = branchRow();
/** What the machine the rail is mounted on offers to start work with. */
let catalog = null;
let calls = [];
let rail = null;

// The rail settles over the disk: its row, and the conversation in it —
// every record it opens is a turn.
const flush = async () => {
  for (let i = 0; i < 12; i++) await new Promise((done) => setTimeout(done, 0));
};

const finishTitleMotion = async (titleElement) => {
  for (let turn = 0; turn < 80 && titleElement.dataset.titleMotion; turn += 1) {
    await new Promise((done) => setTimeout(done, 10));
  }
};

const CATALOG = {
  default_provider: "claude_adk",
  providers: [
    {
      id: "claude_adk",
      label: "Claude Code",
      models: [
        { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
        { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
      ],
      efforts: ["low", "high"],
    },
    { id: "claude", label: "Claude Code TUI", models: [], efforts: [] },
    { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
    { id: "codex", label: "Codex TUI", models: [], efforts: [] },
  ],
};

const railHost = () => document.getElementById("agent-rail");
const modelMenuButton = () => railHost().querySelector(".composer-model .caret");
const menuItem = (action) => railHost().querySelector(`.composer-model .mi[data-action="${action}"]`);
const reasoningMenuButton = () => railHost().querySelector(".composer-reasoning .caret");
const reasoningMenuItem = (action) => railHost().querySelector(`.composer-reasoning .mi[data-action="${action}"]`);
const bubbles = () => [...railHost().querySelectorAll(".rail-bubble")];
const livePainters = () => painters.filter((painter) => !painter.destroyed);
const countOn = (bubble) => bubble.querySelector(".rail-count");
const panel = () => railHost().querySelector(".rail-panel");
/** Whose conversation the head says is open. The head shimmers "Starting" until
 *  the agent names its work, so its title — the topic in full, or the harness
 *  while there is none — is what these cases pin. */
const headWho = (root = railHost()) => root.querySelector(".rail-who").title;
const tuiToggle = () => panel().querySelector(".rail-tui");
const callsTo = (method) => calls.filter((call) => call.method === method);
const railStatus = () => railHost().querySelector("#rail-status");
const railStatusLead = () => railHost().querySelector("#rail-status-lead");
const railStatusPills = () => railHost().querySelector("#rail-status-pills");
const railStatusGit = () => railHost().querySelector("#rail-status-git");
const workingWord = () => railHost().querySelector(".rail-status-working-word");

const pushFeed = async (snapshot) => {
  feedSnapshot = snapshot;
  feedSubscribers.forEach((fn) => fn(snapshot));
  await flush();
};

/** The rail as a surface mounts it: the work item, plus the machine's cache,
 *  conversations and caller, which the view beside it hands down. */
const railAddress = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  projectId: "p1",
  branch: "build/login",
  cacheScope: scopeFor("dev-1"),
  chatRepository,
  call: (method, params) => bridge.call(method, params),
  ...over,
});

const mount = async (context = {}) => {
  await writeRailWorkItem(payload);
  rail = mountAgentRail(railHost(), railAddress(context));
  await flush();
};

/** The row moved: the sync layer writes what the bridge pushed, and every rail
 *  reading that record hears it. */
const pushRow = async (row = payload) => {
  payload = row;
  await writeRailWorkItem(row);
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  resetOptimistic();
  resetDeviceContexts(); // and with them the last test's harness catalog
  await wipeCache();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  calls = [];
  payload = branchRow();
  painters.length = 0;
  feedSubscribers.clear();
  feedSnapshot = { items: [], projects: [] };
  markSeen.mockClear();
  mountAgentTab.mockClear();
  notifyError.mockClear();
  catalog = CATALOG; // asked for once per device; each test gets its own
  bridge.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "models.list") return catalog;
    if (method === "branch.get") return payload;
    if (method === "issue.get") return payload;
    if (method === "run.adopt") return { run_id: "run-9" };
    if (method === "agent.start") return { agent_id: params.agent_id || "ag-new", term_id: `agent:${params.agent_id || "ag-new"}` };
    if (method === "agent.add") return { entity_id: "run-3", agent: agent({ id: "ag-2", ordinal: 2, state: "idle" }) };
    if (method === "thread.post") return { posted_sequence: 7 };
    return {};
  });
  chatRepository = createChatRepository({ scope: scopeFor("dev-1"), call: (method, params) => bridge.call(method, params) });
  // The machine the rail is mounted on: its bridge is what the harness catalog
  // comes from.
  adoptDeviceSession(sessionAnswering(bridge));
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
  chatRepository?.dispose();
  chatRepository = null;
  vi.useRealTimers();
});

// On a phone the panel is not a column beside the work, it is laid over it
// (styles/shell.css, `@media (max-width: 760px)`). A workspace opened at
// 390x844 therefore showed the conversation and nothing else — no Files, no
// Changes, and nothing on screen saying the strip was the way back to them.
// The panel is out by default only where both fit; the reader's own choice,
// once made, holds at either width.
describe("where the conversation panel starts", () => {
  const atWidth = (width) => Object.defineProperty(window, "innerWidth", { configurable: true, value: width });

  afterEach(() => atWidth(1024));

  it("is out beside the work on a desktop and shut over it on a phone", async () => {
    atWidth(1024);
    await mount();
    expect(panel()).toBeTruthy();
    rail.dispose();

    atWidth(390);
    await mount();
    expect(panel()).toBeNull();
  });

  it("opens on the strip's bubble on a phone, and shuts on a second press", async () => {
    atWidth(390);
    await mount();

    bubbles()[0].click();
    await flush();
    expect(panel()).toBeTruthy();

    bubbles()[0].click();
    await flush();
    expect(panel()).toBeTruthy();
    expect(panel().getAttribute("aria-hidden")).toBe("true");
    expect(panel().hasAttribute("inert")).toBe(true);
  });

  it("holds a phone reader's own choice across mounts", async () => {
    atWidth(390);
    await mount();
    bubbles()[0].click();
    await flush();
    // A press on a bubble is a look at one conversation, not a change of mind
    // about the layout: it opens the popover and leaves the choice alone. The
    // pin in the panel's head is what the reader chooses with.
    panel().querySelector(".pinbtn").click();
    await flush();
    rail.dispose();

    await mount();
    expect(panel()).toBeTruthy();
  });
});

// ---- the conversation panel's pin -------------------------------------------
// The panel had one state with no name: a column beside the work, or nothing at
// all, and which of the two you got depended on what was pressed last. It wears
// the inbox's pin now — docked beside the work, or a card on the bubble strip
// pointing at the conversation it belongs to.
describe("the conversation panel's pin", () => {
  const atWidth = (width) => Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  const pin = () => panel().querySelector(".pinbtn");

  afterEach(() => atWidth(1024));

  it("uses the inbox timing while the rail gives workspace width back", () => {
    expect(shellCss).toMatch(/#agent-rail \{[^}]*transition:width 240ms cubic-bezier\(\.2,\.8,\.2,1\)/);
    expect(shellCss).toMatch(/\.rail-panel \{[^}]*border-radius 160ms/);
    expect(shellCss).toMatch(/#agent-rail\.rail-unpinned, #agent-rail\.rail-collapsed \{ width:var\(--agent-strip\); \}/);
    expect(shellCss).toMatch(/#agent-rail, #agent-rail\.rail-unpinned, #agent-rail\.rail-collapsed \{ position:static; width:0; transition:none; \}/);
    expect(shellCss).toMatch(/@media \(prefers-reduced-motion: reduce\) \{[\s\S]*#agent-rail, \.rail-panel \{ transition:none; \}/);
  });

  it("keeps the attachment drop target over the full glass composer", () => {
    expect(shellCss).toMatch(/\.rail-composer \.composer > :not\(\.composer-dropmask\) \{ position:relative; z-index:1; \}/);
    expect(shellCss).toMatch(/\.rail-composer \.composer > \.composer-dropmask \{ position:absolute; inset:0; z-index:2; \}/);
  });

  it("shares the clearer glass values across the header, composer, and activity viewer", () => {
    expect(shellCss).toMatch(/--chat-glass-opacity:72%;/);
    expect(shellCss).toMatch(/--chat-glass-blur:6px;/);
    expect(shellCss.match(/color-mix\(in srgb, var\(--panel\) var\(--chat-glass-opacity\), transparent\)/g)).toHaveLength(3);
    expect(shellCss.match(/backdrop-filter:blur\(var\(--chat-glass-blur\)\)/g)).toHaveLength(6);
  });

  it("says what pressing it does, in the inbox's words", async () => {
    atWidth(1024);
    await mount();
    expect(pin().getAttribute("aria-pressed")).toBe("true");
    expect(pin().title).toBe("Unpin the conversation");
    expect(pin().getAttribute("aria-label")).toBe("Unpin the conversation");
    expect(pin().querySelector("svg")).toBeTruthy();
  });

  it("renders no interaction-blocking scrim", async () => {
    atWidth(1024);
    await mount();
    expect(panel()).toBeTruthy();
    expect(railHost().querySelector("#rail-scrim")).toBeNull();
    expect(railHost().classList.contains("rail-popover")).toBe(false);
    expect(panel().dataset.anchor).toBe("");
  });

  it("unpins to a popover on the strip, and remembers the choice", async () => {
    atWidth(1024);
    await mount();
    pin().click();
    await flush();
    // The conversation stays on screen, as a card over the work rather than a
    // column beside it — the same move the inbox's pin makes.
    expect(railHost().classList.contains("rail-popover")).toBe(true);
    expect(railHost().querySelector("#rail-scrim")).toBeNull();
    expect(pin().getAttribute("aria-pressed")).toBe("false");
    expect(pin().title).toBe("Pin the conversation");
    expect(localStorage.getItem("build.rail.expanded")).toBe("0");

    rail.dispose();
    await mount();
    // Unpinned, a fresh mount is the strip alone until a bubble is pressed.
    expect(panel()).toBeNull();
  });

  it("keeps the live panel, composer, draft, focus, and history position across pin changes", async () => {
    atWidth(1024);
    await mount();
    const standingPanel = panel();
    const input = standingPanel.querySelector("#railinput");
    const history = standingPanel.querySelector(".rail-body");
    input.value = "still drafting";
    Object.defineProperties(history, {
      clientHeight: { configurable: true, value: 100 },
      scrollHeight: { configurable: true, value: 500 },
    });
    history.scrollTop = 37;
    input.focus();

    pin().click();
    await flush();

    expect(panel()).toBe(standingPanel);
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("still drafting");
    expect(document.activeElement).toBe(input);
    expect(history.scrollTop).toBe(37);
  });

  it("pins a popover back into the column", async () => {
    atWidth(390);
    await mount();
    bubbles()[0].click();
    await flush();
    pin().click();
    await flush();
    expect(railHost().classList.contains("rail-popover")).toBe(false);
    expect(railHost().querySelector("#rail-scrim")).toBeNull();
    expect(localStorage.getItem("build.rail.expanded")).toBe("1");

    rail.dispose();
    await mount();
    expect(panel()).toBeTruthy();
  });
});

describe("the unpinned panel's popover", () => {
  const atWidth = (width) => Object.defineProperty(window, "innerWidth", { configurable: true, value: width });

  beforeEach(() => {
    atWidth(390);
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
  });
  afterEach(() => atWidth(1024));

  it("opens on the bubble it was pressed on, and says which one", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    expect(panel().dataset.anchor).toBe("ag-2");
    // Where the notch sits along the panel's edge. jsdom lays nothing out, so
    // every box it measures is at the origin; the browser check is the
    // orchestrator's.
    expect(panel().style.getPropertyValue("--rail-anchor")).toBe("0px");
    expect(railHost().querySelector("#rail-scrim")).toBeNull();
  });

  it("re-anchors on the next bubble rather than closing", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    bubbles()[0].click();
    await flush();
    expect(panel()).toBeTruthy();
    expect(panel().dataset.anchor).toBe("ag-1");
  });

  it("is dismissed by an outside press without consuming the target click", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    const outside = document.createElement("button");
    const clicked = vi.fn();
    outside.onclick = clicked;
    document.body.append(outside);
    const press = new MouseEvent("pointerdown", { bubbles: true, cancelable: true });
    outside.dispatchEvent(press);
    outside.click();
    await flush();
    expect(panel().getAttribute("aria-hidden")).toBe("true");
    expect(press.defaultPrevented).toBe(false);
    expect(clicked).toHaveBeenCalledOnce();
    // Dismissing a popover is not unpinning anything: the choice stands.
    expect(localStorage.getItem("build.rail.expanded")).toBeNull();
  });

  it("leaves the panel's confirmation popover usable", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    panel().querySelector(".rail-remove").click();
    await flush();
    const confirm = document.querySelector(".confirm-popover");
    const cancel = confirm.querySelector("[data-confirm-cancel]");
    cancel.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true }));
    expect(panel().getAttribute("aria-hidden")).toBe("false");
    cancel.click();
    await flush();
    expect(confirm.isConnected).toBe(false);
  });

  it("is dismissed by Escape", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    expect(panel().getAttribute("aria-hidden")).toBe("true");
  });

  it("leaves Escape to a surface that already answered it", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    event.preventDefault();
    document.dispatchEvent(event);
    await flush();
    expect(panel()).toBeTruthy();
  });

  it("is dismissed by navigating", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    window.dispatchEvent(new window.HashChangeEvent("hashchange"));
    await flush();
    expect(panel().getAttribute("aria-hidden")).toBe("true");
  });

  it("does not report reading from the retained panel while it is collapsed", async () => {
    payload = branchRow({
      agents: [agent({ unread_count: 2 })],
      run: { run_id: "run-3", thread: { items: [
        { id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "one" } },
        { id: "m-2", type: "message", data: { sequence: 2, role: "agent", body: "two" } },
      ], sessions: [] } },
    });
    await mount();
    bubbles()[0].click();
    await flush();
    const history = panel().querySelector(".rail-body");
    bubbles()[0].click();
    await flush();
    markSeen.mockClear();

    payload = branchRow({
      agents: [agent({ unread_count: 3 })],
      run: { run_id: "run-3", thread: { items: [
        { id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "one" } },
        { id: "m-2", type: "message", data: { sequence: 2, role: "agent", body: "two" } },
        { id: "m-3", type: "message", data: { sequence: 3, role: "agent", body: "three" } },
      ], sessions: [] } },
    });
    await pushRow();

    expect(markSeen).not.toHaveBeenCalled();
    expect(history.textContent).not.toContain("three");

    bubbles()[0].click();
    await flush();
    expect(history.textContent).toContain("three");
    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 1, 3);
  });

  it("takes its listeners with it when the rail goes", async () => {
    await mount();
    bubbles()[1].click();
    await flush();
    rail.dispose();
    rail = null;
    expect(() =>
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
    ).not.toThrow();
  });
});

// The reviewer's headline complaint: switching workspaces showed a rail with
// nothing in it until a round trip came back. The rail reads the cache and
// nothing else now — the strip and the conversation are on the first frame,
// and a row arriving moves them without anything being asked of the machine.
describe("the rail over a machine that is asked nothing", () => {
  const said = (sequence, body) => ({
    id: `m-${sequence}`,
    type: "message",
    data: { sequence, role: "agent", body, created_at: "2026-08-30T12:00:00Z" },
  });
  const three = () => [agent(), agent({ id: "ag-2", ordinal: 2 }), agent({ id: "ag-3", ordinal: 3 })];

  it("paints the agents and the conversation the disk holds, with no context live at all", async () => {
    payload = branchRow({ agents: three(), run: { run_id: "run-3", thread: { items: [said(1, "what was said")] } } });
    // Nothing is answering: no device session, and every call left hanging.
    resetDeviceContexts();
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      return new Promise(() => {});
    });

    await mount();

    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-1", "ag-2", "ag-3", ""]);
    expect(railHost().querySelector("#rail-body").textContent).toContain("what was said");
  });

  // A rail can be on screen before this machine has a row for the work item at
  // all — the reader deep-linked into a branch, and the first sync pass is
  // still running. The rail stands up on what arrives rather than waiting for
  // something to press.
  it("takes up the row when one arrives for a work item the disk did not hold", async () => {
    await wipeCache();
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    rail = mountAgentRail(railHost(), railAddress());
    await flush();
    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual([""]);

    await writeRailWorkItem(payload);
    await flush();

    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
  });

  it("never takes a painted cold-start conversation down during a sync rewrite", async () => {
    await mount();
    const body = railHost().querySelector("#rail-body");
    expect(body.querySelector(".thread-empty")).toBeTruthy();

    const states = [];
    const observer = new MutationObserver(() => {
      states.push({
        empty: !!body.querySelector(".thread-empty"),
        text: body.textContent,
      });
    });
    observer.observe(body, { childList: true, subtree: true, characterData: true });

    await writeRailThread("run-3", "ag-1", { items: [said(1, "first sync paint")] });
    await flush();
    expect(body.textContent).toContain("first sync paint");
    const paintedAt = states.findIndex((state) => state.text.includes("first sync paint"));
    expect(paintedAt).toBeGreaterThanOrEqual(0);

    // A cache lifetime pass can announce the entity deletion before its
    // ordered thread read writes the replacement window. The live timeline is
    // retained across that gap and then reconciled to the fresh record.
    await evictEntity("dev-1", "run-3");
    await flush();
    expect(body.textContent).toContain("first sync paint");
    await writeRailThread("run-3", "ag-1", {
      items: [said(1, "first sync paint"), said(2, "second sync paint")],
    });
    await flush();

    observer.disconnect();
    expect(states.slice(paintedAt).some((state) => state.empty)).toBe(false);
    expect(body.textContent).toContain("second sync paint");
  });

  it("adds the bubble a row brings without asking anything", async () => {
    payload = branchRow({ agents: three() });
    await mount();
    calls.length = 0;

    await pushRow(branchRow({ agents: [...three(), agent({ id: "ag-4", ordinal: 4 })] }));

    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-1", "ag-2", "ag-3", "ag-4", ""]);
    expect(calls).toEqual([]);
  });

  it("never reads a work item off the wire where the board writes it a row", async () => {
    payload = branchRow({ agents: three() });
    await mount();
    bubbles()[1].click();
    await flush();
    await pushRow(branchRow({ agents: three().slice(0, 2) }));

    const reads = ["branch.get", "issue.get", "run.get", "workspace.get", "thread.page"];
    expect(calls.filter((call) => reads.includes(call.method))).toEqual([]);
  });

  // An issue left the board (bridge board/views.rs), so nothing pushes one a
  // row and nothing ever writes one: the cache holds no work item a rail on an
  // issue could stand on. Its own read is what answers who its agents are, and
  // the rail makes it once — on mount, never on a clock. Stage 9 takes the
  // issue surface cache-only and this goes with it.
  it("asks an issue for its agents, because nothing writes an issue a row", async () => {
    await wipeCache();
    await writeRailThread("plan-1", "ag-1", { items: [said(1, "on the issue")] });
    payload = { issue_id: "plan-1", project_id: "p1", agents: [agent()], thread: { items: [said(1, "on the issue")] } };
    rail = mountAgentRail(railHost(), railAddress({ kind: "issue", projectId: "p1", issueId: "plan-1" }));
    await flush();

    // One bubble and no `+`: an issue carries exactly one agent.
    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-1"]);
    expect(railHost().querySelector("#rail-body").textContent).toContain("on the issue");
    expect(callsTo("issue.get")).toHaveLength(1);
    expect(calls.filter((call) => ["branch.get", "run.get", "workspace.get"].includes(call.method))).toEqual([]);
  });
});

describe("the bubble strip", () => {
  it("is one bubble per agent plus the one that adds another", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2, unread_count: 4, working: true })] });
    await mount();
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["agent", "agent", "add"]);
    expect(countOn(bubbles()[1]).textContent).toBe("4");
    expect(bubbles()[1].classList.contains("working")).toBe(true);
  });

  it("opens the agent it is pressed on, and closes on a second press", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    bubbles()[1].click();
    await flush();
    expect(headWho(panel())).toBe("Polish the rail");
    expect(bubbles()[1].classList.contains("active")).toBe(true);
    bubbles()[1].click();
    await flush();
    expect(panel().getAttribute("aria-hidden")).toBe("true");
    // …and the strip is still there with the panel shut.
    expect(bubbles().length).toBe(3);
  });

  it("collapses and restores the same pinned panel without changing the pin choice", async () => {
    await mount();
    const standing = panel();
    const input = standing.querySelector("#railinput");
    input.value = "kept draft";
    input.focus();

    bubbles()[0].click();
    await flush();
    expect(panel()).toBe(standing);
    expect(panel().getAttribute("aria-hidden")).toBe("true");
    expect(bubbles()[0].getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(bubbles()[0]);
    expect(localStorage.getItem("build.rail.expanded")).toBeNull();

    bubbles()[0].click();
    await flush();
    expect(panel()).toBe(standing);
    expect(panel().getAttribute("aria-hidden")).toBe("false");
    expect(panel().querySelector("#railinput").value).toBe("kept draft");
    expect(bubbles()[0].getAttribute("aria-expanded")).toBe("true");
    expect(railHost().classList.contains("rail-unpinned")).toBe(false);
  });

  // A file dragged onto another agent's bubble is for that agent: the drop
  // opens its conversation with the file already in the box, ready to send.
  it("takes a file dropped on a bubble into that agent's composer", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    expect(headWho(panel())).toBe("Fix login redirect");
    const png = new File(["png"], "shot.png", { type: "image/png" });
    const over = new Event("dragover", { bubbles: true, cancelable: true });
    over.dataTransfer = { files: [], items: [], types: ["Files"], dropEffect: "none" };
    bubbles()[1].dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    expect(bubbles()[1].classList.contains("is-dropping")).toBe(true);
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    drop.dataTransfer = { files: [png], items: [], types: ["Files"] };
    bubbles()[1].dispatchEvent(drop);
    await flush();
    expect(drop.defaultPrevented).toBe(true);
    expect(bubbles()[1].classList.contains("is-dropping")).toBe(false);
    expect(headWho(panel())).toBe("Polish the rail");
    expect(bubbles()[1].classList.contains("active")).toBe(true);
    expect(panel().getAttribute("aria-hidden")).toBe("false");
    const chips = [...panel().querySelectorAll(".composer-chip")];
    expect(chips.map((chip) => chip.querySelector(".composer-chip-name").textContent)).toEqual(["shot.png"]);
    expect(callsTo("thread.attach")[0].params).toMatchObject({ entity_id: "run-3", filename: "shot.png" });
  });

  it("ignores a file dropped on the + bubble", async () => {
    await mount();
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    drop.dataTransfer = { files: [new File(["x"], "x.txt")], items: [], types: ["Files"] };
    railHost().querySelector('[data-bubble="add"]').dispatchEvent(drop);
    await flush();
    expect(drop.defaultPrevented).toBe(false);
    expect(panel().querySelectorAll(".composer-chip")).toHaveLength(0);
    expect(callsTo("thread.attach")).toEqual([]);
  });

  it("repaints the header icon when a provider update arrives", async () => {
    await mount();
    expect(panel().querySelector(".rail-harness-icon").dataset.harnessIcon).toBe("claude_adk");

    payload = branchRow({ agents: [agent({ provider: "codex_app_server", topic: "" })] });
    await pushRow();

    expect(panel().querySelector(".rail-harness-icon").dataset.harnessIcon).toBe("codex_app_server");
    // Nothing named yet, so the hover falls back to the harness — the new one.
    expect(headWho(panel())).toBe("Codex");
  });

  // The rail reads the row every 1.6s and nearly every read says the same
  // thing. A rewrite then swaps the button a press is landing on for an
  // identical one, and the press is swallowed.
  it("leaves the bubbles alone on a tick that reads the same agents", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const before = bubbles();

    await pushRow();

    const after = bubbles();
    expect(after).toHaveLength(before.length);
    expect(after.every((bubble, index) => bubble === before[index])).toBe(true);
  });

  // What a bubble SAYS — its unread count, whether it is working, which one is
  // open — is written onto the button that is already there. The element only
  // changes when the AGENTS do. That is what lets a paused pattern animation
  // keep the frame it stopped on: replacing the element would restart it.
  it("writes a bubble's news onto the button already there, rather than replacing it", async () => {
    await mount();
    const before = bubbles()[0];
    payload = branchRow({ agents: [agent({ unread_count: 3, working: true })] });

    await pushRow();

    expect(bubbles()[0]).toBe(before);
    expect(countOn(bubbles()[0]).textContent).toBe("3");
    expect(countOn(bubbles()[0]).hidden).toBe(false);
    expect(bubbles()[0].classList.contains("working")).toBe(true);
    expect(bubbles()[0].title).toContain("3 unread");
    expect(bubbles()[0].getAttribute("aria-label")).toContain("3 unread");

    // …and back again: the count goes, the animation stops where it was.
    payload = branchRow({ agents: [agent()] });
    await pushRow();
    expect(bubbles()[0]).toBe(before);
    expect(countOn(bubbles()[0]).hidden).toBe(true);
    expect(bubbles()[0].classList.contains("working")).toBe(false);
  });

  it("keeps each bubble's element when another agent joins the strip", async () => {
    await mount();
    const before = bubbles()[0];
    const painter = livePainters()[0];
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });

    await pushRow();

    expect(bubbles()).toHaveLength(3);
    expect(bubbles()[0]).toBe(before);
    expect(painter.destroyed).toBe(false);
  });

  it("wears a painted pattern instead of a number", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const [first, second, add] = bubbles();
    expect(first.querySelector("canvas.rail-glyph")).toBeTruthy();
    expect(first.dataset.pattern).toBe("1");
    expect(second.dataset.pattern).toBe("2");
    expect(first.textContent.trim()).toBe("");
    // The `+` speaks in a glyph, not a pattern, so it gets no canvas at all.
    expect(add.querySelector("canvas")).toBe(null);
    expect(add.textContent.trim()).toBe("+");
    // The name is still said where a name belongs — the tooltip and the
    // accessible name — and it is the work the agent named itself, not a
    // harness and a number.
    expect(first.getAttribute("aria-label")).toBe("Fix login redirect");
    expect(second.getAttribute("aria-label")).toBe("Polish the rail");
  });

  // The corner badge is gone: an unread count is the whole face now, over a
  // pattern dropped back far enough to read it against.
  it("carries no corner badge", async () => {
    payload = branchRow({ agents: [agent({ unread_count: 2 })] });
    await mount();
    expect(railHost().querySelector(".rail-badge")).toBe(null);
  });

  it("shows a single ghost where no agent has been born yet", async () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
    await mount();
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["ghost"]);
  });

  it("offers no second agent on an issue", async () => {
    payload = { issue_id: "plan-1", project_id: "p1", agents: [agent()], thread: { items: [] } };
    await mount({ kind: "issue", projectId: "p1", issueId: "plan-1" });
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["agent"]);
  });

  // The reviewer's screenshot: pressing + created an agent silently on the
  // stored default and landed in its empty chat — no harness selector. The +
  // opens the chooser now, seeded with the stored preference; the send is
  // what creates, exactly as on a branch with no agents at all.
  it("opens the harness chooser instead of creating an agent outright", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({ provider: "codex", model: "", effort: "" }));
    await mount();
    railHost().querySelector('[data-bubble="add"]').click();
    await flush();
    expect(callsTo("agent.add")).toEqual([]);
    const chooser = railHost().querySelector(".rail-newagent");
    expect(chooser).toBeTruthy();
    expect(chooser.querySelector(".rail-harness-choice.chosen").dataset.provider).toBe("codex");
    expect(headWho()).toBe("New agent");
    expect(railHost().querySelector("#railinput").placeholder).toContain("start an agent");
  });

  it("creates on the first message, with the chosen harness, and opens the new chat", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({ provider: "codex", model: "", effort: "" }));
    await mount();
    railHost().querySelector('[data-bubble="add"]').click();
    await flush();
    railHost().querySelector("#railinput").value = "start here";
    railHost().querySelector("#railsend").click();
    await flush();
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-3", provider: "codex" });
    expect(callsTo("thread.post")[0].params).toMatchObject({ entity_id: "run-3", agent_id: "ag-2", body: "start here" });
    expect(railHost().querySelector(".rail-newagent")).toBeNull();
  });

  // A browser-local preference naming the carrier no surface offers any more.
  // It is a record, and the offer is where it clamps — the chooser highlights
  // the agent every other surface would have created, not one nobody can pick.
  it("clamps a stored preference for the other claude carrier in the chooser", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({ provider: "claude", model: "", effort: "" }));
    await mount();
    railHost().querySelector('[data-bubble="add"]').click();
    await flush();
    expect(railHost().querySelector(".rail-newagent .rail-harness-choice.chosen").dataset.provider).toBe("claude_adk");
  });

  it("backs out of the chooser onto whichever bubble is pressed", async () => {
    await mount();
    railHost().querySelector('[data-bubble="add"]').click();
    await flush();
    expect(railHost().querySelector(".rail-newagent")).toBeTruthy();
    bubbles()[0].click();
    await flush();
    expect(railHost().querySelector(".rail-newagent")).toBeNull();
    expect(callsTo("agent.add")).toEqual([]);
  });

  it("publishes which agent is open, so the surfaces beside it ask about the same one", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    const selection = createAgentSelection();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", selection });
    // The rail opens on the first agent, and the surfaces beside it hear so.
    expect(selection.get()).toBe("ag-1");
    await pushRow();
    expect(selection.get()).toBe("ag-1");

    bubbles()[1].click();
    await flush();
    expect(selection.get()).toBe("ag-2");
    // A row arriving does not move the reader off the bubble they opened.
    await pushRow();
    expect(selection.get()).toBe("ag-2");
  });

  it("lets go of an agent the work item no longer has, instead of holding a bubble nobody answers for", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    const selection = createAgentSelection();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", selection });
    bubbles()[1].click();
    await flush();
    expect(selection.get()).toBe("ag-2");

    // The run behind the branch was replaced: its row names another agent, and
    // the one the reader had open is not on the work item any more.
    await pushRow(branchRow({ agents: [agent()] }));

    expect(selection.get()).toBe("ag-1");
    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-1", ""]);
  });
});

// The bubble's face is painted, not styled: core/agentCanvas.js draws a tiling
// into the canvas and the rail owns one painter per bubble. What the rail owes
// it is a lifecycle and three switches — working, ink, dimmed.
describe("the painter behind a bubble", () => {
  it("makes one painter per patterned bubble, on the pattern that bubble wears", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    expect(livePainters().map((painter) => painter.options.patternIndex)).toEqual([1, 2]);
  });

  // The same agent has to look like itself for as long as the tab is open, and
  // like something else the next time it is opened — so the seed is the agent's
  // id turned by a salt that lives as long as the page does.
  it("seeds each agent differently, and the same agent the same way all session", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const [first, second] = livePainters().map((painter) => painter.options.seed);
    expect(first).not.toBe(second);

    rail.dispose();
    painters.length = 0;
    await mount();
    expect(livePainters()[0].options.seed).toBe(first);
  });

  it("runs the painter while the agent works and stops it when it does not", async () => {
    await mount();
    expect(livePainters()[0].working).toBe(false);

    payload = branchRow({ agents: [agent({ working: true })] });
    await pushRow();
    expect(livePainters()[0].working).toBe(true);

    payload = branchRow({ agents: [agent()] });
    await pushRow();
    expect(livePainters()[0].working).toBe(false);
  });

  // An unread count is drawn ON the face: the pattern drops back, turns amber,
  // and the number sits centred over it.
  it("turns the face amber and dim under an unread count, and back again", async () => {
    await mount();
    const painter = livePainters()[0];
    const resting = painter.ink;
    expect(painter.dimmed).toBe(false);
    expect(countOn(bubbles()[0]).hidden).toBe(true);

    payload = branchRow({ agents: [agent({ unread_count: 2 })] });
    await pushRow();
    expect(painter.dimmed).toBe(true);
    expect(painter.ink).toBeTruthy();
    expect(painter.ink).not.toBe(resting);
    expect(countOn(bubbles()[0]).hidden).toBe(false);
    expect(countOn(bubbles()[0]).textContent).toBe("2");

    payload = branchRow({ agents: [agent()] });
    await pushRow();
    expect(painter.dimmed).toBe(false);
    expect(painter.ink).toBe(resting);
    expect(countOn(bubbles()[0]).hidden).toBe(true);
  });

  // Same invariant as the buttons themselves: a painter is made when its
  // element is, and a tick that only changes what a bubble SAYS must not make
  // another — that would rewind the pattern to its first frame.
  it("keeps the same painter through a tick that only changes the news", async () => {
    await mount();
    const painter = livePainters()[0];
    payload = branchRow({ agents: [agent({ working: true, unread_count: 1 })] });

    await pushRow();

    expect(livePainters()).toEqual([painter]);
  });

  it("lets go of the painter for an agent that left the strip", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const [, second] = livePainters();

    payload = branchRow({ agents: [agent()] });
    await pushRow();

    expect(second.destroyed).toBe(true);
    expect(livePainters()).toHaveLength(1);
  });

  it("lets go of every painter when the rail comes down", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    expect(livePainters()).toHaveLength(2);

    rail.dispose();
    rail = null;
    expect(livePainters()).toHaveLength(0);
  });
});

describe("taking an agent back off the branch", () => {
  const twoAgents = (over = {}) => branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })], ...over });
  const removeButton = () => panel().querySelector(".rail-remove");
  const confirmModal = () => document.querySelector(".confirm-popover");

  const openSecondAgent = async () => {
    payload = twoAgents();
    await mount();
    bubbles()[1].click();
    await flush();
  };

  const holdRemove = () => {
    const answering = bridge.call;
    let refuse = null;
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "agent.remove") return new Promise((_, reject) => { refuse = reject; });
      return answering(method, params);
    });
    return { refuse: (error) => refuse(error) };
  };
  const railBodyNow = () => railHost().querySelector("#rail-body");
  const confirmEveryModal = () => {
    document.querySelectorAll(".modal-scrim, .confirm-popover").forEach((dialog) => {
      const ok = dialog.querySelector("[data-confirm-ok]");
      if (ok) ok.click();
    });
  };

  it("offers removal on every agent, the first and the only one included", async () => {
    payload = twoAgents();
    await mount();
    // The rail opens on the first agent, and that one may go too.
    expect(removeButton()).toBeTruthy();
    bubbles()[1].click();
    await flush();
    expect(removeButton()).toBeTruthy();
  });

  it("leaves a branch whose last agent went with the view that asks for a new one", async () => {
    await mount();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    payload = branchRow({ agents: [] });
    await flush();
    // An answer that loses the agents has to say it twice — a poll hiccup must
    // not close the conversation under a reader. A real remove-all says it
    // every tick.
    await pushRow();

    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-1" });
    // A working branch, not a broken one: the strip drops to its ghost and the
    // panel head offers to start a new agent.
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["ghost"]);
    expect(headWho(panel())).toBe("New agent");
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("offers no removal on an issue's one agent", async () => {
    payload = { issue_id: "plan-1", project_id: "p1", agents: [agent()], thread: { items: [] } };
    await mount({ kind: "issue", projectId: "p1", issueId: "plan-1" });
    expect(removeButton()).toBe(null);
  });

  // The button says what it will take away in the agent's own words, and the
  // question behind it says the same ones.
  it("names the agent it removes by the topic that agent set", async () => {
    await openSecondAgent();
    expect(removeButton().title).toBe('Remove "Polish the rail" from this branch');
    expect(removeButton().getAttribute("aria-label")).toBe('Remove "Polish the rail" from this branch');
    removeButton().click();
    await flush();
    expect(confirmModal().textContent).toContain('Remove "Polish the rail" from this branch?');
  });

  // A topic arriving keeps the head mounted — it is the title that moves, not
  // the controls — so the button re-reads its wording on the title's beat
  // rather than waiting for a rebuild that has no reason to happen.
  it("points at an agent that has named nothing yet, and picks the name up when it arrives", async () => {
    payload = branchRow({ agents: [agent({ topic: "" })] });
    await mount();
    const standing = removeButton();
    expect(standing.title).toBe("Remove this Claude Code agent from this branch");

    payload = branchRow({ agents: [agent({ topic: "Unify prompt delivery" })] });
    await pushRow();

    expect(removeButton()).toBe(standing);
    expect(standing.title).toBe('Remove "Unify prompt delivery" from this branch');
    expect(standing.getAttribute("aria-label")).toBe('Remove "Unify prompt delivery" from this branch');
  });

  // Two agents on one harness with nothing named yet read the same in every
  // word the head has: the panel is open on an id, not on a name, and the
  // button takes away the agent whose bubble is lit.
  it("removes the agent the strip is lit on, even beside its twin", async () => {
    payload = branchRow({ agents: [agent({ topic: "" }), agent({ id: "ag-2", ordinal: 2, topic: "" })] });
    await mount();
    expect(headWho(panel())).toBe("Claude Code");
    bubbles()[1].click();
    await flush();
    expect(headWho(panel())).toBe("Claude Code");

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-2" });
  });

  it("asks before it removes, and does nothing at all when the answer is no", async () => {
    await openSecondAgent();
    removeButton().click();
    await flush();
    expect(confirmModal()).toBeTruthy();
    expect(document.getElementById("confirm-scrim")).toBeNull();
    confirmModal().querySelector("[data-confirm-cancel]").click();
    await flush();
    expect(callsTo("agent.remove")).toEqual([]);
  });

  it("cancels the question when the conversation changes", async () => {
    await openSecondAgent();
    removeButton().click();
    await flush();
    expect(confirmModal()).toBeTruthy();

    bubbles()[0].click();
    await flush();

    expect(confirmModal()).toBeNull();
    expect(headWho(panel())).toBe("Fix login redirect");
    expect(callsTo("agent.remove")).toEqual([]);
  });

  it("cancels the question when the rail is disposed", async () => {
    await mount();
    removeButton().click();
    await flush();
    rail.dispose();
    rail = null;
    await flush();

    expect(confirmModal()).toBeNull();
    expect(callsTo("agent.remove")).toEqual([]);
  });

  it("removes the agent the panel is open on, and falls back to the one that is left", async () => {
    await openSecondAgent();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    // The agent is gone from the work item the next read answers with.
    payload = branchRow();
    await flush();

    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-2" });
    expect(headWho(panel())).toBe("Fix login redirect");
    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", ""]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("tells which agent it removed to the surfaces beside it, so they stop asking after it", async () => {
    payload = twoAgents();
    const selection = createAgentSelection();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", selection });
    bubbles()[1].click();
    await flush();
    expect(selection.get()).toBe("ag-2");

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    payload = branchRow();
    await flush();
    expect(selection.get()).toBe("ag-1");
  });

  // An older bridge binary has no agent.remove at all. The rail must say so the
  // standard way and stay exactly as it was, not break under the refusal.
  it("raises the standard error notice when the daemon does not know the method", async () => {
    await openSecondAgent();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "agent.remove") throw new Error("unknown method: agent.remove");
      return answering(method, params);
    });

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    expect(notifyError).toHaveBeenCalledWith("Could not remove the agent", "unknown method: agent.remove");
    // Still open on the agent it failed to remove, still offering to try again.
    expect(headWho(panel())).toBe("Polish the rail");
    expect(removeButton()).toBeTruthy();
    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
  });

  it("takes the agent off the moment the answer is yes, before the daemon replies", async () => {
    payload = twoAgents();
    // Only the second agent has said anything, so dropping to the first is a
    // different conversation and an empty one.
    await writeRailThread("run-3", "ag-2", {
      items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "words from the second agent" } }],
    });
    await mount();
    bubbles()[1].click();
    await flush();
    expect(railBodyNow().querySelectorAll(".thread-body")).toHaveLength(1);
    holdRemove();

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    expect(payload.agents.map((each) => each.id)).toEqual(["ag-1", "ag-2"]);
    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", ""]);
    expect(headWho(panel())).toBe("Fix login redirect");
    expect(railBodyNow().querySelectorAll(".thread-body")).toHaveLength(0);
    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-2" });
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("drops to the ghost when the agent it took off was the only one, before the daemon replies", async () => {
    await mount();
    holdRemove();

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    expect(payload.agents.map((each) => each.id)).toEqual(["ag-1"]);
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["ghost"]);
    expect(headWho(panel())).toBe("New agent");
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("does not resurrect the agent when a row lands mid-flight still listing it", async () => {
    await openSecondAgent();
    holdRemove();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    // The board has not caught up with a removal the daemon has not answered
    // for yet; the reader is not shown the bubble they just took off.
    await pushRow(twoAgents());

    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", ""]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("puts the agent and its conversation back when the daemon refuses, and says so once", async () => {
    payload = twoAgents({
      run: {
        run_id: "run-3",
        thread: { items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "words from the second agent" } }], sessions: [] },
      },
    });
    await mount();
    bubbles()[1].click();
    await flush();
    expect(railBodyNow().querySelectorAll(".thread-body")).toHaveLength(1);
    const held = holdRemove();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();
    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", ""]);

    held.refuse(new Error("agent is mid-spawn"));
    await flush();

    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
    expect(headWho(panel())).toBe("Polish the rail");
    expect(railBodyNow().querySelectorAll(".thread-body")).toHaveLength(1);
    expect(removeButton()).toBeTruthy();
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("Could not remove the agent", "agent is mid-spawn");
  });

  it("stands the refused removal back up without waiting for a read that never answers", async () => {
    await openSecondAgent();
    const held = holdRemove();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "branch.get") throw new Error("the bridge is not answering");
      return answering(method, params);
    });

    held.refuse(new Error("agent is mid-spawn"));
    await flush();

    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
    expect(bubbles()[1].classList.contains("active")).toBe(true);
    expect(headWho(panel())).toBe("Polish the rail");
  });

  it("removes an agent the create record still names", async () => {
    payload = branchRow({ agents: [] });
    await mount();
    panel().querySelector("#railinput").value = "start here";
    panel().querySelector("#railsend").click();
    await flush();
    expect(bubbles()[0].dataset.agent).toBe("ag-2");

    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();

    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-2" });
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["ghost"]);
  });

  it("refuses a second removal of the same agent while the first is in flight", async () => {
    await openSecondAgent();
    holdRemove();

    removeButton().click();
    removeButton().click();
    await flush();
    confirmEveryModal();
    await flush();

    expect(callsTo("agent.remove")).toHaveLength(1);
  });
});

describe("the conversation panel", () => {
  it("animates only a changed topic while preserving the live header and its focused controls", async () => {
    payload = branchRow({ agents: [agent({ topic: "" })] });
    await mount();
    // Nothing named yet: the head says so, in the shimmering word, and the
    // hover falls back to the harness.
    const standingHead = panel().querySelector(".rail-head");
    const standingTitle = standingHead.querySelector(".rail-who");
    const standingPin = standingHead.querySelector(".pinbtn");
    expect(standingTitle.textContent).toBe("Starting");
    expect(standingTitle.classList.contains("rail-who-starting")).toBe(true);
    expect(headWho(panel())).toBe("Claude Code");
    standingPin.focus();

    // An unchanged poll neither rebuilds the head nor starts title motion.
    vi.advanceTimersByTime(1600);
    await flush();
    expect(panel().querySelector(".rail-head")).toBe(standingHead);
    expect(standingTitle.dataset.titleMotion).toBeUndefined();
    expect(document.activeElement).toBe(standingPin);

    payload = branchRow({ agents: [agent({ topic: "Unify prompt delivery" })] });
    await pushRow();

    expect(panel().querySelector(".rail-head")).toBe(standingHead);
    expect(panel().querySelector(".pinbtn")).toBe(standingPin);
    expect(panel().querySelector(".rail-who")).toBe(standingTitle);
    expect(document.activeElement).toBe(standingPin);
    expect(standingTitle.dataset.titleMotion).toBe("erasing");

    await finishTitleMotion(standingTitle);
    expect(standingTitle.textContent).toBe("Unify prompt delivery");
    expect(standingTitle.classList.contains("rail-who-starting")).toBe(false);
    expect(headWho(panel())).toBe("Unify prompt delivery");
  });

  it("carries the agent, the one way down to its screen, and a box to write in", async () => {
    await mount();
    expect(headWho(panel())).toBe("Fix login redirect");
    const modes = [...panel().querySelectorAll(".rail-mode")];
    expect(modes).toHaveLength(1);
    expect(modes[0].textContent).toBe("TUI");
    expect(modes[0].getAttribute("aria-pressed")).toBe("false");
    expect(modes[0].classList.contains("on")).toBe(false);
    expect(modes[0].title).toBe("Show the terminal");
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  it("offers no Chat chip and no mode group: the panel already lives in the conversation", async () => {
    await mount();
    expect(railHost().querySelector('[data-mode="chat"]')).toBe(null);
    expect(railHost().querySelector(".rail-modes")).toBe(null);
  });

  it("drops to the screen and comes back on the same button", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    expect(panel().querySelector("#railinput")).toBe(null);
    expect(tuiToggle().getAttribute("aria-pressed")).toBe("true");
    expect(tuiToggle().classList.contains("on")).toBe(true);
    expect(tuiToggle().title).toBe("Back to the conversation");

    tuiToggle().click();
    await flush();
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(tuiToggle().getAttribute("aria-pressed")).toBe("false");
    expect(tuiToggle().classList.contains("on")).toBe(false);
  });

  it("keeps the terminal mounted when returning to the browser tab", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    const body = railHost().querySelector("#rail-body");
    const terminal = document.createElement("div");
    terminal.textContent = "live terminal";
    body.append(terminal);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(body.contains(terminal)).toBe(true);
    expect(body.querySelector(".thread-items")).toBeNull();
    expect(tuiToggle().getAttribute("aria-pressed")).toBe("true");
  });

  it("remembers the face per work item, reopening on the screen the reader left the branch on", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    rail.dispose();

    await mount();
    expect(tuiToggle().getAttribute("aria-pressed")).toBe("true");
    expect(panel().querySelector("#railinput")).toBe(null);
  });

  it("swaps the same panel onto the agent's screen, addressed by that agent", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    expect(mountAgentTab).toHaveBeenCalled();
    expect(mountAgentTab.mock.calls[0][1]).toEqual({ id: "run-3", agent_id: "ag-1" });
    expect(panel().querySelector("#railinput")).toBe(null);
  });

  // The pane's offer is Resume, and resuming names no harness: the agent is
  // locked to the one it was created on, and its conversation is waiting there.
  it("resumes the agent the pane belongs to, on the harness it already has", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();

    await mountAgentTab.mock.calls[0][2].onStart();

    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-3", agent_id: "ag-1" });
  });

  // The daemon answers a start before the harness exists, so the reply cannot
  // say whether one came up. The bubble wears `starting` from the press until
  // the entity's own answer says the session is live.
  it("wears starting from the press until the entity says the session is live", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();

    await mountAgentTab.mock.calls[0][2].onStart();

    expect(bubbles()[0].classList.contains("starting")).toBe(true);
    expect(bubbles()[0].title).toContain("starting…");

    payload = branchRow({ agents: [agent({ state: "live" })] });
    await pushRow();

    expect(bubbles()[0].classList.contains("starting")).toBe(false);
  });

  // The other answer to a start. A spawn that never came up says nothing about
  // a session, so waiting on `live` alone left the ring on for the overlay's
  // whole 30 s grace and then dropped it silently, with the reason nowhere.
  it("takes the ring off and says why when the entity reports the start failed", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();

    await mountAgentTab.mock.calls[0][2].onStart();
    expect(bubbles()[0].classList.contains("starting")).toBe(true);

    payload = branchRow({
      agents: [agent({ state: "idle", start_error: "could not reach the agent: no such worktree" })],
    });
    await pushRow();

    expect(bubbles()[0].classList.contains("starting")).toBe(false);
    expect(bubbles()[0].title).toContain("could not reach the agent");
    expect(notifyError).toHaveBeenCalledWith(
      "Could not start the agent",
      "could not reach the agent: no such worktree",
    );
  });

  // A start that names no agent is still a start: the entity names the agent it
  // opened on its next answer, and the rail reads it there.
  it("carries on when the start answers without naming the agent it opened", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.start") {
        calls.push({ method, params });
        return {};
      }
      return answering(method, params);
    });

    await mountAgentTab.mock.calls[0][2].onStart();

    expect(notifyError).not.toHaveBeenCalled();
    expect(bubbles()[0].dataset.agent).toBe("ag-1");
    expect(bubbles()[0].classList.contains("starting")).toBe(true);
  });

  it("marks the agent starting the instant Resume is pressed, so a message behind it starts nothing twice", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.start") {
        calls.push({ method, params });
        return new Promise(() => {});
      }
      return answering(method, params);
    });

    mountAgentTab.mock.calls[0][2].onStart();
    await flush();
    tuiToggle().click();
    await flush();
    panel().querySelector("#railinput").value = "carry on";
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("thread.post")).toHaveLength(1);
    expect(callsTo("agent.start")).toHaveLength(1);
  });

  // The common way a cold agent is started is a message, not the Resume press:
  // the post wakes the agent behind it. That row wears the same starting state
  // from the send, which is also what keeps a second send behind it from
  // opening a second harness.
  it("marks the agent starting from a message that wakes it, so a second message starts nothing twice", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.start") {
        calls.push({ method, params });
        return new Promise(() => {});
      }
      return answering(method, params);
    });

    panel().querySelector("#railinput").value = "wake up";
    panel().querySelector("#railsend").click();
    await flush();
    expect(bubbles()[0].classList.contains("starting")).toBe(true);

    panel().querySelector("#railinput").value = "and carry on";
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("thread.post")).toHaveLength(2);
    expect(callsTo("agent.start")).toHaveLength(1);
    expect(bubbles()[0].classList.contains("starting")).toBe(true);
  });

  // The third answer to a start: the entity's session is over, so there is no
  // agent to open. The daemon says so on the agent, and the ring comes off the
  // way it does for a spawn that failed.
  it("takes the ring off a message-started agent when the entity says no session will open", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();

    panel().querySelector("#railinput").value = "wake up";
    panel().querySelector("#railsend").click();
    await flush();
    expect(bubbles()[0].classList.contains("starting")).toBe(true);

    payload = branchRow({
      agents: [agent({ state: "exited", start_error: "no session to open: this entity's session is over" })],
    });
    await pushRow();

    expect(bubbles()[0].classList.contains("starting")).toBe(false);
    expect(bubbles()[0].title).toContain("no session to open");
    expect(notifyError).toHaveBeenCalledWith(
      "Could not start the agent",
      "no session to open: this entity's session is over",
    );
  });

  // A start the browser stopped waiting for is not a refusal: the daemon is
  // spawning the harness behind the answer it already gave. Painting "could not
  // start the agent" over a session that is coming up is the same confusion the
  // other verbs were fixed for.
  it("leaves the bubble starting and says nothing when the start outlives the timer", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.start") {
        calls.push({ method, params });
        const timedOut = new Error("agent.start timed out");
        timedOut.timedOut = true;
        timedOut.uncertain = true;
        throw timedOut;
      }
      return answering(method, params);
    });

    await expect(mountAgentTab.mock.calls[0][2].onStart()).resolves.toBeNull();

    expect(notifyError).not.toHaveBeenCalled();
    expect(bubbles()[0].classList.contains("starting")).toBe(true);
  });

  it("puts the agent back where it was and says why when the start is refused", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    tuiToggle().click();
    await flush();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.start") {
        calls.push({ method, params });
        throw new Error("no session could be spawned");
      }
      return answering(method, params);
    });

    await expect(mountAgentTab.mock.calls[0][2].onStart()).rejects.toThrow("no session could be spawned");
    expect(notifyError).not.toHaveBeenCalled();

    tuiToggle().click();
    await flush();
    panel().querySelector("#railinput").value = "try again";
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("agent.start")).toHaveLength(2);
  });

  // The terminal is a capability, not a guarantee. A harness that reports its
  // own reasoning and tool calls is not opaque, so it has no basement to drop
  // into — and the rail is where that shows: no TUI button, and no way to ask
  // for one.
  it("offers the terminal only to an agent whose session has one", async () => {
    payload = branchRow({ agents: [agent({ has_terminal: false })] });
    await mount();
    expect(panel().querySelectorAll(".rail-mode")).toHaveLength(0);
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  // An older bridge does not mention the field at all, and silence is not a
  // refusal: every agent had a terminal before this question could be asked.
  it("keeps the terminal for a digest that never mentions one", async () => {
    await mount();
    expect(agent().has_terminal).toBe(undefined);
    expect(panel().querySelectorAll(".rail-mode")).toHaveLength(1);
  });

  // The face the panel wears is remembered per work item, so opening a
  // terminal-less agent's bubble arrives with "tui" in hand. It must land on
  // the conversation anyway, and attach nothing.
  it("puts the panel back on the conversation when a terminal-less agent is opened", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2, has_terminal: false })] });
    await mount();
    tuiToggle().click();
    await flush();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);

    bubbles()[1].click();
    await flush();

    expect(headWho(panel())).toBe("Polish the rail");
    expect(panel().querySelectorAll(".rail-mode")).toHaveLength(0);
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);

    // …and the choice is not spent: the agent that does have a terminal is
    // still where it was left.
    bubbles()[0].click();
    await flush();
    expect(tuiToggle()).toBeTruthy();
    expect(mountAgentTab).toHaveBeenCalledTimes(2);
  });

  // The digest can change its answer under a panel that is already open — an
  // agent is replaced by one of another shape on the same bubble. The screen
  // has to go with it.
  it("takes the terminal away from a panel standing on one when the agent loses it", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    expect(panel().querySelector("#railinput")).toBe(null);

    payload = branchRow({ agents: [agent({ has_terminal: false })] });
    await pushRow();

    expect(panel().querySelector(".rail-tui")).toBe(null);
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  it("leaves a live screen alone while the rail keeps polling", async () => {
    await mount();
    tuiToggle().click();
    await flush();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);
    const before = panel();
    vi.advanceTimersByTime(5000);
    await flush();
    // The same panel element, the same pane: a poll must not re-attach a PTY.
    expect(panel()).toBe(before);
    expect(mountAgentTab).toHaveBeenCalledTimes(1);
  });

  it("tells the daemon how far down an agent's conversation it has read", async () => {
    payload = branchRow({
      agents: [agent({ unread_count: 2, unread_reason: "done" })],
      run: {
        run_id: "run-3",
        thread: {
          sessions: [],
          items: [
            { type: "message", data: { sequence: 11, role: "agent", body: "asked" } },
            { type: "message", data: { sequence: 12, role: "agent", body: "and asked again" } },
          ],
        },
      },
    });
    await mount();
    // The floor is the oldest message the panel holds, and 12 is the newest its
    // viewport reached — which over a conversation that arrived whole is all of
    // it.
    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 11, 12);
  });

  it("says nothing about reading a conversation with nothing waiting", async () => {
    await mount();
    expect(markSeen).not.toHaveBeenCalled();
  });

  /// A conversation the reader has been away from, with the daemon's cursor
  /// saying where they got to.
  const conversationReadThrough = (cursor, unreadCount) => {
    payload = branchRow({
      agents: [agent({ unread_count: unreadCount, unread_reason: "agent_message", read_through_sequence: cursor })],
      run: {
        run_id: "run-3",
        thread: {
          sessions: [],
          items: [
            { type: "message", data: { sequence: 11, role: "agent", body: "the one you read" } },
            { type: "message", data: { sequence: 12, role: "agent", body: "the one you did not" } },
          ],
        },
      },
    });
  };

  it("rules a line above the first message the reader has not read", async () => {
    conversationReadThrough(11, 1);
    await mount();

    const line = railHost().querySelector(".thread-unread-line");
    expect(line).toBeTruthy();
    expect(line.nextElementSibling.textContent).toContain("the one you did not");
  });

  it("keeps New for 60 seconds after reading the latest agent reply", async () => {
    conversationReadThrough(11, 1);
    const geometry = vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function () {
      return { top: 0, bottom: this.getAttribute("data-key") === "12" ? 100 : 0, height: 0, width: 0 };
    });
    try {
      await mount();
    } finally {
      geometry.mockRestore();
    }
    vi.useFakeTimers();
    const body = railHost().querySelector("#rail-body");
    body.onscroll();
    vi.advanceTimersByTime(59_999);
    expect(railHost().querySelector(".thread-unread-line")).toBeTruthy();
    vi.advanceTimersByTime(1);
    expect(railHost().querySelector(".thread-unread-line")).toBeNull();
  });

  it("clears New when leaving mid-grace, including with a stale digest on reopening", async () => {
    conversationReadThrough(11, 1);
    await mount();
    expect(railHost().querySelector(".thread-unread-line")).toBeTruthy();
    bubbles()[0].click();
    await flush();
    bubbles()[0].click();
    await flush();
    expect(railHost().querySelector(".thread-unread-line")).toBeNull();
  });

  it("rules no line over a conversation with nothing waiting in it", async () => {
    conversationReadThrough(12, 0);
    await mount();

    expect(railHost().querySelector(".thread-unread-line")).toBeNull();
  });

  // The reviewer's bug: pressing the second bubble moved the selection and the
  // remove button, and left the first agent's conversation on screen. The panel
  // waited for the poll to bring the other conversation — and with the bridge
  // pushing change events, that poll has stood down to a 60s safety read, so the
  // wrong words sat there for a minute.
  describe("switching between two agents", () => {
    const said = (who) => ({
      items: [{ type: "message", data: { sequence: 1, role: "agent", body: `words from ${who}`, seen_at: null } }],
      sessions: [],
    });

    /** Two conversations on one work item, each in its own record — which is
     *  how they are stored, and why pressing a bubble needs nothing off the
     *  wire to show what was said in it. */
    const twoAgents = async () => {
      payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
      await writeRailThread("run-3", "ag-1", said("ag-1"));
      await writeRailThread("run-3", "ag-2", said("ag-2"));
    };

    it("shows the conversation of the agent it switched to, off the disk, asking nothing", async () => {
      await twoAgents();
      await mount();
      expect(panel().textContent).toContain("words from ag-1");
      const asked = calls.length;

      bubbles()[1].click();
      await flush();

      expect(headWho(panel())).toBe("Polish the rail");
      expect(panel().textContent).toContain("words from ag-2");
      expect(panel().textContent).not.toContain("words from ag-1");
      expect(calls.length).toBe(asked);
    });

    it("never shows one agent's words under another's name while the record is being opened", async () => {
      await twoAgents();
      await mount();
      expect(panel().textContent).toContain("words from ag-1");

      // The press repaints before the record can be read. Whatever the panel
      // draws in that gap, it must not be the conversation of the agent left.
      bubbles()[1].click();
      expect(panel().textContent).not.toContain("words from ag-1");

      await flush();
      expect(panel().textContent).toContain("words from ag-2");
    });
  });
});

describe("reading back past the top of a paged conversation", () => {
  // A long conversation reaches the client as a page of its newest items, so
  // the top of the scroller is a floor rather than the start of anything. The
  // reader hitting it is the ask for the page above.
  const said = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
  const firstPage = (hasMore) => ({
    sessions: [],
    items: [said(98, "second to last"), said(99, "the newest")],
    thread_total: 99,
    thread_last_sequence: 99,
    oldest_sequence: 98,
    has_more: hasMore,
  });
  const pageAbove = (hasMore) => ({
    items: [said(96, "the oldest we asked for"), said(97, "one before the window")],
    thread_total: 99,
    thread_last_sequence: 99,
    oldest_sequence: 96,
    has_more: hasMore,
  });
  const railBody = () => railHost().querySelector("#rail-body");

  // What the sync layer left on disk: a window over the newest items, saying
  // whether the conversation reaches back further than it does.
  const pagedConversation = (hasMore, moreAboveThatPage = true, agents = [agent()]) => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return catalog;
      if (method === "thread.page") return pageAbove(moreAboveThatPage);
      return {};
    });
    payload = branchRow({ agents, run: { run_id: "run-3", thread: firstPage(hasMore) } });
  };

  it("asks the daemon for the page above the window when the reader reaches the top", async () => {
    pagedConversation(true);
    await mount();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(callsTo("thread.page").map((call) => call.params)).toEqual([
      { entity_id: "run-3", agent_id: "ag-1", before_sequence: 98 },
    ]);
  });

  it("folds the older items in above the ones already on screen", async () => {
    pagedConversation(true);
    await mount();
    expect(panel().textContent).not.toContain("one before the window");

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    const bodies = [...railBody().querySelectorAll(".thread-body")].map((node) => node.textContent);
    expect(bodies).toEqual([
      "the oldest we asked for",
      "one before the window",
      "second to last",
      "the newest",
    ]);
  });

  it("asks nothing when the window already holds the start of the conversation", async () => {
    pagedConversation(false);
    await mount();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(callsTo("thread.page")).toEqual([]);
  });

  it("stops asking once a page answers that the window holds the start", async () => {
    // The page above is the start of the conversation, so there is nothing
    // left to fetch. The repaint that draws it folds the first-load page back
    // through the cache on its way — and that page still says there is more
    // above a floor the reader has now scrolled past.
    pagedConversation(true, false);
    await mount();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();
    expect(callsTo("thread.page")).toHaveLength(1);

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(callsTo("thread.page")).toHaveLength(1);
  });

  // The reviewer's bug: the scroller sitting at the bottom used to mean the
  // whole conversation had been shipped and could be read through. It now
  // means the reader reached the end of a window, so the read report says
  // where that window starts and the daemon keeps the badge up for a message
  // waiting below it.
  it("reports how much of the conversation it holds when it reports it read", async () => {
    pagedConversation(true, true, [agent({ unread_count: 1, unread_reason: "agent_message" })]);
    await mount();

    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 98, 99);
  });

  it("moves the floor it reports down as the reader scrolls back", async () => {
    // A report the daemon dropped for history it could not vouch for is worth
    // making again once that history has landed, so a window reaching further
    // back is news even when the reader got no further down.
    pagedConversation(true, true, [agent({ unread_count: 1, unread_reason: "agent_message" })]);
    await mount();
    markSeen.mockClear();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 96, 99);
  });

  it("asks once for a page, however many scroll events the gesture fires", async () => {
    pagedConversation(true);
    await mount();

    railBody().dispatchEvent(new Event("scroll"));
    railBody().dispatchEvent(new Event("scroll"));
    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(callsTo("thread.page")).toHaveLength(1);
  });
});

// The number on a folded run is the bridge's count of the whole run, and the
// window is what carries it: a page says what each run totals, and every later
// paint — a poll's delta, a repaint of the payload in hand, the window read
// back after an agent switch — draws the same number rather than counting the
// rows that happen to be on screen.
describe("the count on a folded run of activity", () => {
  const toolCall = (sequence, summary) => ({
    type: "event",
    data: { sequence, event: "tool_use", summary, created_at: "2026-09-06T18:03:11.412Z" },
  });
  const digest = (from, through, toolCalls, lastToolCall = null) => ({
    from_sequence: from,
    through_sequence: through,
    tool_calls: toolCalls,
    rows: toolCalls,
    last_tool_call: lastToolCall,
  });
  const railBody = () => railHost().querySelector("#rail-body");
  const foldCount = () => railBody().querySelector(".thread-activity-count").textContent;

  const bigRun = (over = {}) => ({
    sessions: [],
    items: [toolCall(1529, "Read spa/src/core/thread.js"), toolCall(1530, "Bash(cargo test)")],
    activity_digests: [digest(412, 1530, 1000, { sequence: 1530, summary: "Bash(cargo test)", outcome: "ok" })],
    thread_total: 1530,
    thread_last_sequence: 1530,
    oldest_sequence: 1529,
    has_more: true,
    ...over,
  });

  const conversationOf = (thread) => {
    payload = branchRow({ run: { run_id: "run-3", thread } });
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "thread.page") {
        return {
          items: [toolCall(400, "Read bridge/src/thread.rs")],
          activity_digests: [digest(300, 411, 40, { sequence: 400, summary: "Read bridge/src/thread.rs" })],
          thread_total: 1530,
          thread_last_sequence: 1530,
          oldest_sequence: 400,
          has_more: false,
        };
      }
      return {};
    });
  };

  it("shows what the bridge counted rather than the rows the page shipped", async () => {
    conversationOf(bigRun());
    await mount();

    expect(foldCount()).toBe("1000");
    expect(railBody().querySelector(".thread-activity-preview").textContent).toBe("Bash(cargo test)");
  });

  it("keeps the count through the repaints a poll and a feed snapshot make", async () => {
    conversationOf(bigRun());
    await mount();

    await pushFeed({ items: [], projects: [] });
    expect(foldCount()).toBe("1000");
  });

  it("takes in the digest of the run an older page reaches back to", async () => {
    conversationOf(bigRun());
    await mount();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    // One run on screen now: the page above ended in a tool call, so what the
    // reader sees is a single fold over both bridge runs, counting both.
    expect(railBody().querySelectorAll(".thread-activity-group")).toHaveLength(1);
    expect(foldCount()).toBe("1040");
  });
});

describe("focusing the composer on a freshly created branch", () => {
  // A branch fresh out of "New branch…" has no agent yet — the ghost state —
  // same as every test in this block below.
  const freshBranch = () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
  };

  it("focuses the composer as soon as it paints, when the view says to", async () => {
    freshBranch();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", autofocusComposer: true });
    expect(document.activeElement).toBe(panel().querySelector("#railinput"));
  });

  it("leaves focus alone on an ordinary mount", async () => {
    freshBranch();
    await mount();
    expect(document.activeElement).not.toBe(panel().querySelector("#railinput"));
  });

  it("expands a rail the human had collapsed, so there is a composer to focus at all", async () => {
    freshBranch();
    localStorage.setItem("build.rail.expanded", "0");
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", autofocusComposer: true });
    expect(panel()).toBeTruthy();
    expect(document.activeElement).toBe(panel().querySelector("#railinput"));
  });

  it("steals focus only once, not on every poll that repaints the same body", async () => {
    freshBranch();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", autofocusComposer: true });
    panel().querySelector("#railinput").blur();
    expect(document.activeElement).not.toBe(panel().querySelector("#railinput"));

    vi.advanceTimersByTime(1600);
    await flush();
    expect(document.activeElement).not.toBe(panel().querySelector("#railinput"));
  });
});

describe("the pinned status line above the composer", () => {
  // Every machine mints a `p1` with a `build/login` in it, and the feed holds
  // all of them at once. The rail is about one work item on one machine, so the
  // row it reports on comes out of that machine's slice.
  it("the rail reads the work item's row from its own device's slice of the feed", async () => {
    const theirs = {
      kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-2",
      working: false, working_time: null, stat: { insertions: 4, deletions: 1, ahead: 2, behind: 0 },
    };
    const mine = {
      kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
      working: false, working_time: null, stat: { insertions: 99, deletions: 99, ahead: 0, behind: 0 },
    };
    await pushFeed({
      items: [mine, theirs],
      projects: [],
      devices: {
        "dev-1": { items: [mine], projects: [] },
        "dev-2": { items: [theirs], projects: [] },
      },
    });

    await mount({ deviceId: "dev-2" });

    expect(railStatus().textContent).toContain("+4");
    expect(railStatus().textContent).toContain("−1");
    expect(railStatus().textContent).not.toContain("99");
  });

  it("pins nothing when the feed row has nothing to report", async () => {
    await mount();
    expect(railStatus().hidden).toBe(true);
    expect(railStatus().textContent.trim()).toBe("");
  });

  it("pulses and clocks the turn while the branch is working", async () => {
    payload = branchRow({ agents: [agent({ working_time: { since: new Date(Date.now() - 750000).toISOString(), seconds: 750 } })] });
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: true, working_time: { since: new Date(Date.now() - 750000).toISOString(), seconds: 750 }, stat: null,
      }],
      projects: [],
    });
    await mount();
    expect(railStatus().hidden).toBe(false);
    expect(railStatusLead().className).toBe("rail-status-lead rail-status-working");
    expect(railStatusLead().textContent).toBe("Working 12:30");
  });

  it("shows the diffstat and ahead/behind alongside, only when nonzero", async () => {
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: false, working_time: null, stat: { insertions: 4, deletions: 1, ahead: 2, behind: 0 },
      }],
      projects: [],
    });
    await mount();
    expect(railStatusLead().hidden).toBe(true);
    expect(railStatus().textContent).toContain("+4");
    expect(railStatus().textContent).toContain("−1");
    expect(railStatus().textContent).toContain("↑2");
    expect(railStatus().textContent).not.toContain("↓");
  });

  it("holds the git facts in one right-anchored group, clear of the timer's wobble", async () => {
    // The reviewer's screenshot: the sync and diffstat sat right after the
    // timer, so every tick that widened it ("9m 59s" → "10m 0s") shoved them
    // around. One group carries both, and the sheet anchors it to the row's end.
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: true, working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 },
        stat: { insertions: 104, deletions: 38, ahead: 0, behind: 1 },
      }],
      projects: [],
    });
    await mount();
    const git = railStatus().querySelector(".rail-status-git");
    expect(git).toBeTruthy();
    expect(git.textContent).toBe("↓1+104−38");
    expect([...git.children].map((cell) => cell.getAttribute("data-cell"))).toEqual([
      "behind:glyph", "behind:d0",
      "insertions:glyph", "insertions:d2", "insertions:d1", "insertions:d0",
      "deletions:glyph", "deletions:d1", "deletions:d0",
    ]);
    expect(railStatus().lastElementChild).toBe(git);
    expect(railStatus().firstElementChild).toBe(railStatusLead());
  });

  it("shows the startup event in the working slot while no turn is in flight", async () => {
    payload = branchRow({
      run: {
        run_id: "run-3",
        thread: {
          sessions: [],
          items: [{ type: "event", data: { event: "run_started", created_at: new Date(Date.now() - 120000).toISOString(), sequence: 1 } }],
        },
      },
    });
    await mount();
    expect(railStatus().hidden).toBe(false);
    expect(railStatus().textContent).toContain("Run started · 2m ago");
    expect(railStatusLead().className).toBe("rail-status-lead rail-status-starting");

    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: true, working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 }, stat: null,
      }],
      projects: [],
    });
    payload = branchRow({
      ...payload,
      agents: [agent({ ...payload.agents[0], working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 } })],
    });
    await pushRow();
    expect(railStatus().textContent).toContain("Working 0:05");
    expect(railStatus().textContent).not.toContain("Run started");
    expect(railStatusLead().className).toBe("rail-status-lead rail-status-working");
  });

  it("names the session's start in the harness that raised it", async () => {
    payload = branchRow({
      agents: [agent({ provider: "codex" })],
      run: {
        run_id: "run-3",
        thread: {
          sessions: [],
          items: [{ type: "event", data: { event: "session_started", created_at: new Date(Date.now() - 120000).toISOString(), sequence: 1 } }],
        },
      },
    });
    await mount();
    expect(railStatus().textContent).toContain("Codex TUI session started");
  });

  it("ticks the elapsed time between feed reads", async () => {
    // The ticker's Date.now() has to move with the fake clock for this one, so
    // this test fakes Date too — the others read `since` off the real clock at
    // mount and never advance timers far enough to notice the difference.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    payload = branchRow({ agents: [agent({ working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 } })] });
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: true, working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 }, stat: null,
      }],
      projects: [],
    });
    await mount();
    expect(railStatus().textContent).toContain("Working 0:05");
    vi.advanceTimersByTime(3000);
    await flush();
    expect(railStatus().textContent).toContain("Working 0:08");
  });
});

describe("the first message", () => {
  it("posts to the agent that is there, and starts it when no session is live", async () => {
    payload = branchRow({ agents: [agent({ state: "idle" })] });
    await mount();
    panel().querySelector("#railinput").value = "please look at this";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3", agent_id: "ag-1", body: "please look at this",
    });
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-3", agent_id: "ag-1" });
  });

  // The answer to a post is a whole entity view, conversation and all, and
  // nothing here reads it — the refresh that follows is what paints. The page
  // is asked for anyway, because an answer nobody reads must still not grow
  // with the conversation, and asking for none fetches all of it.
  it("names a page on the post it is about to throw away", async () => {
    payload = branchRow({ agents: [agent({ state: "idle" })] });
    await mount();
    panel().querySelector("#railinput").value = "one more word";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("thread.post")[0].params.thread_limit).toBe(FIRST_PAGE_ITEMS);
  });

  it("says nothing twice to an agent already listening", async () => {
    await mount();
    panel().querySelector("#railinput").value = "hi";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("agent.start")).toEqual([]);
  });

  // The rail is one of two surfaces on a branch that can mutate first, so the
  // view above it owns the adopter and hands it down. Adopting on its own here
  // would mint a second owner of the checkout the Changes review just claimed.
  it("adopts through the adopter the view hands it", async () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
    const shared = createAdoptingCall((method, params) => bridge.call(method, params), "p1", "wt-3");
    await shared.adopt();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", adopting: () => shared });

    panel().querySelector("#railinput").value = "start here";
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("run.adopt")).toHaveLength(1);
    expect(callsTo("thread.post")[0].params).toMatchObject({ entity_id: "run-9", body: "start here" });
  });

  it("leaves an issue's first message to start its own planning agent", async () => {
    payload = { issue_id: "plan-1", project_id: "p1", agents: [agent({ state: "idle" })], thread: { items: [] } };
    await mount({ kind: "issue", projectId: "p1", issueId: "plan-1" });
    panel().querySelector("#railinput").value = "plan this";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("thread.post")[0].params).toMatchObject({ entity_id: "plan-1", agent_id: "ag-1" });
    expect(callsTo("agent.start")).toEqual([]);
  });
});

// A branch starts with no agents at all, and so does one whose agents were all
// removed. Its chat tab is where an agent is created: the harnesses on offer,
// the account's default already highlighted, over the same composer every
// conversation has. Sending is the one act that creates one and speaks to it.
describe("the chat tab of a branch with no agent", () => {
  const agentless = () => branchRow({ agents: [] });
  const cards = () => [...railHost().querySelectorAll(".rail-newagent .rail-harness-choice")];
  const card = (provider) => cards().find((entry) => entry.dataset.provider === provider);
  const chosenCard = () => cards().find((entry) => entry.classList.contains("chosen"));
  const send = async (body) => {
    panel().querySelector("#railinput").value = body;
    panel().querySelector("#railsend").click();
    await flush();
  };

  it("offers the two agents, with the account's default already chosen", async () => {
    payload = agentless();
    await mount();

    expect(cards().map((entry) => entry.dataset.provider)).toEqual(["claude_adk", "codex"]);
    expect(cards().map((entry) => entry.textContent.trim())).toEqual(["Claude Code", "Codex"]);
    expect(chosenCard().dataset.provider).toBe("claude_adk");
    // No conversation to show: there is no agent whose conversation it would be.
    expect(railHost().querySelector(".thread-items")).toBe(null);
    expect(panel().querySelector("#railinput").placeholder).toContain("start an agent");
  });

  it("creates the chosen agent, delivers to it, starts it, and opens its bubble", async () => {
    payload = agentless();
    await mount();

    await send("start here");

    expect(calls.filter((call) => call.method.startsWith("agent.") || call.method === "thread.post")
      .map((call) => call.method)).toEqual(["agent.add", "thread.post", "agent.start"]);
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-3", provider: "claude_adk" });
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3", agent_id: "ag-2", body: "start here",
    });
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-3", agent_id: "ag-2" });

    // The branch has the agent the send made, and the panel is open on it.
    payload = branchRow({ agents: [agent({ id: "ag-2", ordinal: 2 })] });
    await pushRow();
    expect(headWho(panel())).toBe("Polish the rail");
  });

  it("moves the highlight to the card that is pressed, and creates that one", async () => {
    payload = agentless();
    await mount();

    card("codex").click();
    await flush();
    expect(chosenCard().dataset.provider).toBe("codex");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-3", provider: "codex" });
  });

  it("keeps model and effort when the selected harness is pressed again", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "claude_adk", model: "claude-opus-5", effort: "high",
    }));
    payload = agentless();
    await mount();

    chosenCard().click();
    await flush();
    expect(document.activeElement).toBe(chosenCard());

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      provider: "claude_adk", model: "claude-opus-5", effort: "high",
    });
  });

  // Each harness has a preference of its own on the account page, so moving
  // the highlight brings that harness's model and effort with it rather than
  // starting from nothing — and the composer's menu says so at once.
  it("seeds a pressed harness with its own saved model and effort", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "codex",
      harnesses: { claude: { model: "claude-opus-5", effort: "high" }, codex: { model: "", effort: "" } },
    }));
    payload = agentless();
    await mount();
    expect(chosenCard().dataset.provider).toBe("codex");

    card("claude_adk").click();
    await flush();
    expect(chosenCard().dataset.provider).toBe("claude_adk");
    expect(modelMenuButton().textContent).toContain("Claude Opus 5");
    expect(reasoningMenuButton().textContent).toContain("high");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      provider: "claude_adk", model: "claude-opus-5", effort: "high",
    });
  });

  it("starts on the catalog's default harness with that harness's saved preference when none is chosen", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "",
      harnesses: { claude: { model: "claude-opus-5", effort: "high" } },
    }));
    payload = agentless();
    await mount();
    expect(chosenCard().dataset.provider).toBe("claude_adk");
    expect(modelMenuButton().textContent).toContain("Claude Opus 5");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      provider: "claude_adk", model: "claude-opus-5", effort: "high",
    });
  });

  it("passes a custom saved model through for a compatible harness", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "claude_adk", model: "company-custom-model", effort: "custom",
    }));
    payload = agentless();
    await mount();

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      provider: "claude_adk", model: "company-custom-model", effort: "custom",
    });
  });

  it("adopts a checkout Build owns nothing in before it creates the agent", async () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
    await mount();

    await send("start here");

    expect(callsTo("run.adopt")[0].params).toMatchObject({ project_id: "p1", worktree_id: "wt-3" });
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-9", provider: "claude_adk" });
    expect(callsTo("thread.post")[0].params).toMatchObject({ entity_id: "run-9", agent_id: "ag-2", body: "start here" });
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-9", agent_id: "ag-2" });
  });

  it("holds the model the menu chose until the send that creates the agent", async () => {
    payload = agentless();
    await mount();

    modelMenuButton().click();
    menuItem("model:claude-opus-5").click();
    await flush();
    // Nothing on the wire: there is no agent yet to hold a choice, and this
    // checkout may never be adopted at all.
    expect(callsTo("agent.choose")).toEqual([]);
    expect(modelMenuButton().textContent).toContain("Claude Opus 5");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      entity_id: "run-3", provider: "claude_adk", model: "claude-opus-5",
    });
  });

  // The account decides which carrier "Claude Code" means, and the card is
  // where that answer lands: one name, whichever carrier is behind it.
  it("gives the Claude Code card the carrier the account chose", async () => {
    catalog = { ...CATALOG, default_provider: "claude" };
    payload = agentless();
    await mount();

    expect(cards().map((entry) => entry.dataset.provider)).toEqual(["claude", "codex"]);
    expect(cards().map((entry) => entry.textContent.trim())).toEqual(["Claude Code", "Codex"]);
    expect(chosenCard().dataset.provider).toBe("claude");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-3", provider: "claude" });
  });

  // A choice held from before the account moved its default. The offer is where
  // a stale token clamps, so the view highlights the card it is showing —
  // never nothing at all.
  it("clamps a choice made under the other claude carrier onto the card on offer", async () => {
    catalog = { ...CATALOG, default_provider: "claude" };
    payload = agentless();
    await mount();
    card("claude").click();
    await flush();

    rail.dispose();
    catalog = CATALOG;
    await contextFor("dev-1").refreshModelCatalog();
    await mount();

    expect(cards().map((entry) => entry.dataset.provider)).toEqual(["claude_adk", "codex"]);
    expect(cards().filter((entry) => entry.classList.contains("chosen"))).toHaveLength(1);
    expect(chosenCard().dataset.provider).toBe("claude_adk");

    await send("start here");
    expect(callsTo("agent.add")[0].params).toMatchObject({ entity_id: "run-3", provider: "claude_adk" });
  });

  it("leaves the cards alone on a tick that says the same thing", async () => {
    payload = agentless();
    await mount();
    const before = cards();

    await pushRow();

    expect(cards().every((entry, index) => entry === before[index])).toBe(true);
  });
});

// The model menu sits on the composer's left, opposite the send. It edits what
// the agent's NEXT turn runs on — a live session keeps what it opened with —
// and it never asks which harness: the agent is locked to the one it was made
// on, so the menu asks only what is still a question.
describe("the composer's model menu", () => {
  it("offers the open agent's own catalog, and says what the next turn runs on", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5", effort: "low" })] });
    await mount();

    expect(modelMenuButton().textContent).toContain("Claude Opus 5");
    expect(reasoningMenuButton().textContent).toContain("low");
    modelMenuButton().click();
    expect([...railHost().querySelectorAll(".composer-model .mi")].map((mi) => mi.dataset.action)).toEqual([
      "model:claude-opus-5", "model:claude-haiku-4-5",
    ]);
    reasoningMenuButton().click();
    expect([...railHost().querySelectorAll(".composer-reasoning .mi")].map((mi) => mi.dataset.action)).toEqual([
      "effort:low", "effort:high",
    ]);
  });

  it("persists the choice on the entity, saying nothing about the harness", async () => {
    payload = branchRow({ agents: [agent()] });
    await mount();

    modelMenuButton().click();
    menuItem("model:claude-opus-5").click();
    await flush();

    expect(callsTo("agent.choose")[0].params).toEqual({
      entity_id: "run-3", agent_id: "ag-1", model: "claude-opus-5", effort: "", expected_choice_revision: 0,
    });
  });

  it("keeps the effort put on top of a pick the bridge has not caught up with", async () => {
    payload = branchRow({ agents: [agent({ model: "", effort: "" })] });
    await mount();

    modelMenuButton().click();
    menuItem("model:claude-opus-5").click();
    await flush();
    reasoningMenuButton().click();
    reasoningMenuItem("effort:high").click();
    await flush();

    payload = branchRow({ agents: [agent({ model: "claude-opus-5", effort: "high" })] });
    await pushRow();

    expect(modelMenuButton().textContent).toContain("Claude Opus 5");
    expect(reasoningMenuButton().textContent).toContain("high");
  });

  it("says the model the open agent is actually running on, with no second round trip", async () => {
    payload = branchRow({ agents: [agent({ model: "", effort: "", active_model: "claude-opus-5", active_effort: "high" })] });
    await mount();

    expect(modelMenuButton().textContent).toContain("Claude Opus 5");
    expect(reasoningMenuButton().textContent).toContain("high");
    expect(callsTo("agent.choose")).toEqual([]);
  });

  it("names the pending model beside it once the menu has chosen another", async () => {
    payload = branchRow({ agents: [agent({ model: "", effort: "", active_model: "claude-opus-5" })] });
    await mount();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.choose") {
        calls.push({ method, params });
        payload = branchRow({
          agents: [agent({ model: params.model, effort: "", active_model: "claude-opus-5" })],
        });
        return {};
      }
      return answering(method, params);
    });

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();
    await flush();

    expect(callsTo("agent.choose")[0].params).toEqual({
      entity_id: "run-3", agent_id: "ag-1", model: "claude-haiku-4-5", effort: "", expected_choice_revision: 0,
    });
    expect(modelMenuButton().textContent).toContain("Claude Opus 5 → Claude Haiku 4.5");
    expect(menuItem("model:claude-haiku-4-5").className).toContain("on");
  });

  it("says the harness's defaults stand when an agent has never run and chose nothing", async () => {
    payload = branchRow({ agents: [agent({ model: "", effort: "", active_model: "" })] });
    await mount();

    expect(modelMenuButton().textContent).toContain("Harness default");
    expect(reasoningMenuButton().textContent).toContain("Default effort");
  });

  it("moves the label the instant a model is picked, before agent.choose answers", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5" })] });
    await mount();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.choose") {
        calls.push({ method, params });
        return new Promise(() => {});
      }
      return answering(method, params);
    });

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();

    expect(modelMenuButton().textContent).toContain("Claude Haiku 4.5");
    expect(callsTo("agent.choose")).toHaveLength(1);
    await flush();
    expect(modelMenuButton().textContent).toContain("Claude Haiku 4.5");
  });

  it("does not send with the old model while a newly picked model is still applying", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5" })] });
    await mount();
    const answering = bridge.call;
    let acknowledgeChoice;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.choose") {
        calls.push({ method, params });
        return new Promise((resolve) => { acknowledgeChoice = resolve; });
      }
      return answering(method, params);
    });

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();
    const composer = panel().querySelector("#railinput");
    composer.value = "use the new model";
    composer.dispatchEvent(new Event("input", { bubbles: true }));
    panel().querySelector("#railsend").click();

    expect(callsTo("thread.post")).toEqual([]);
    expect(panel().querySelector("#railsend").disabled).toBe(true);
    expect(panel().querySelector("#railhint").textContent).toContain("Applying model…");

    acknowledgeChoice({
      entity_id: "run-3",
      agent_id: "ag-1",
      provider: "claude_adk",
      model: "claude-haiku-4-5",
      effort: "",
      choice_revision: 1,
    });
    await flush();
    expect(panel().querySelector("#railsend").disabled).toBe(false);
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("thread.post")[0].params).toMatchObject({
      agent_id: "ag-1",
      choice_revision: 1,
      body: "use the new model",
    });
  });

  it("uses model and reasoning changes made in an existing conversation on the next send", async () => {
    payload = branchRow({
      agents: [agent({ model: "claude-opus-5", effort: "low", active_model: "claude-opus-5" })],
    });
    await mount();

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();
    await flush();
    modelMenuButton().click();
    menuItem("model:claude-opus-5").click();
    await flush();
    reasoningMenuButton().click();
    reasoningMenuItem("effort:high").click();
    await flush();

    const composer = panel().querySelector("#railinput");
    composer.value = "continue with these settings";
    composer.dispatchEvent(new Event("input", { bubbles: true }));
    panel().querySelector("#railsend").click();
    await flush();

    expect(callsTo("agent.choose").at(-1).params).toMatchObject({
      agent_id: "ag-1",
      model: "claude-opus-5",
      effort: "high",
    });
    expect(callsTo("thread.post")[0].params).toMatchObject({
      agent_id: "ag-1",
      choice_revision: 3,
      body: "continue with these settings",
    });
  });

  it("keeps the pick through a branch.get that still names the old model", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5" })] });
    await mount();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method === "agent.choose") {
        calls.push({ method, params });
        return new Promise(() => {});
      }
      return answering(method, params);
    });

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();
    await flush();

    vi.advanceTimersByTime(1600);
    await flush();

    expect(modelMenuButton().textContent).toContain("Claude Haiku 4.5");
    modelMenuButton().click();
    expect(menuItem("model:claude-haiku-4-5").className).toContain("on");
  });

  it("says a refusal the standard way and puts the menu back on what the bridge holds", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5" })] });
    await mount();
    const answering = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "agent.choose") throw new Error("agent.choose: the agent is locked to Claude Code");
      return answering(method, params);
    });

    modelMenuButton().click();
    menuItem("model:claude-haiku-4-5").click();
    await flush();

    expect(notifyError).toHaveBeenCalledWith(
      "Could not set the model",
      "agent.choose: the agent is locked to Claude Code",
    );
    expect(modelMenuButton().textContent).toContain("Claude Opus 5");
  });
});

// An empty composer stops an interruptible active turn. As soon as there is a
// draft, the same stable control sends it instead.
describe("interrupting the turn", () => {
  const send = () => panel().querySelector("#railsend");

  it("offers the plain send to an agent whose turn cannot be stopped", async () => {
    payload = branchRow({ agents: [agent({ working: true })] });
    await mount();
    expect(send().dataset.action).toBe("send");
  });

  it("offers the plain send to an agent that is not working, whatever it can do", async () => {
    payload = branchRow({ agents: [agent({ working: false, can_interrupt: true })] });
    await mount();
    expect(send().dataset.action).toBe("send");
  });

  it("shows stop for a working agent that announced the interrupt", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    expect(send().dataset.action).toBe("stop");
    expect(send().getAttribute("aria-label")).toBe("Stop agent");
  });

  it("invokes the standalone interrupt when stop is pressed", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    send().click();
    await flush();
    expect(callsTo("agent.interrupt")[0].params).toMatchObject({
      entity_id: "run-3", agent_id: "ag-1", conversation_id: "ag-1",
    });
  });

  it("changes stop back to send as soon as the user types", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    const input = panel().querySelector("#railinput");
    input.value = "when you get a moment";
    input.dispatchEvent(new Event("input"));
    expect(send().dataset.action).toBe("send");
    send().click();
    await flush();
    expect(callsTo("thread.post")[0].params.body).toBe("when you get a moment");
    expect(callsTo("agent.interrupt")).toHaveLength(0);
  });

  // The condition changes every time an agent starts or finishes a turn, which
  // on a 1.6s poll is often. Rebuilding the composer to swap the control would
  // take the draft and the focus with it, mid-sentence.
  it("swaps the control in place, keeping the box and the words in it", async () => {
    payload = branchRow({ agents: [agent({ working: true })] });
    await mount();
    const input = panel().querySelector("#railinput");
    input.value = "half a sent";
    expect(send().dataset.action).toBe("send");

    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await pushRow();

    expect(send().dataset.action).toBe("send");
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("half a sent");

    // …and back to the plain button when the turn it could have stopped ends.
    payload = branchRow({ agents: [agent({ working: false, can_interrupt: true })] });
    await pushRow();
    expect(send().dataset.action).toBe("send");
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("half a sent");
  });
});

describe("the conversation's local cache", () => {
  const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3", deviceId: "dev-1" }];
  const threadItem = (sequence, body) => ({
    id: `m-${sequence}`,
    type: "message",
    data: { sequence, role: "user", body, created_at: "2026-08-30T12:00:00Z" },
  });

  it("opens the saved window, and asks the bridge for nothing at all", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
      { items: [threadItem(1, "what was said before")], olderItemsRemain: false, deliveredSequence: 1, knownTotalItems: 1 },
    );
    feedSnapshot = { items: feedItems, projects: [] };
    await mount();
    await flush(); // the auto-selected agent's record is opened

    expect(railHost().querySelector("#rail-body").textContent).toContain("what was said before");
    // The conversation is on the disk; the wire is only ever asked for the
    // harnesses this machine offers.
    expect(calls.map((call) => call.method)).toEqual(["models.list"]);
  });

  // The count on a folded run comes off the window, so the window on disk
  // carries it: a reader coming back to a long run sees the bridge's number on
  // the seeded paint, not a count of the handful of rows the disk held.
  it("seeds the digests with the window, so the fold's count survives the visit", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
      {
        items: [{
          id: "e-1530",
          type: "event",
          data: { sequence: 1530, event: "tool_use", summary: "Bash(cargo test)", created_at: "2026-08-30T12:00:00Z" },
        }],
        olderItemsRemain: true,
        deliveredSequence: 1530,
        knownTotalItems: 1530,
        activityDigests: [{ from_sequence: 412, through_sequence: 1530, tool_calls: 1000, rows: 1000, last_tool_call: null }],
      },
    );
    feedSnapshot = { items: feedItems, projects: [] };
    await mount();
    await flush();

    expect(railHost().querySelector(".thread-activity-count").textContent).toBe("1000");
  });

  // The panel writes to the record only for what the reader does: a message
  // they send stands on it until the wire carries it back. Everything else in
  // there is the sync layer's.
  it("writes what the reader sends into the window, and nothing else", async () => {
    feedSnapshot = { items: feedItems, projects: [] };
    payload = branchRow({
      run: {
        run_id: "run-3",
        thread: { items: [threadItem(2, "fresh words")], has_more: false, thread_total: 1, thread_last_sequence: 2, sessions: [] },
      },
    });
    await mount();
    panel().querySelector("#railinput").value = "and mine";
    panel().querySelector("#railsend").click();
    await flush();

    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" });
    expect(record.value.items.map((entry) => entry.data.body)).toEqual(["fresh words", "and mine"]);
    expect(record.value.items[1].data.sequence).toBe(7); // the sequence thread.post answered with
  });
});

describe("revisiting a conversation", () => {
  const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3", deviceId: "dev-1" }];
  const historyThread = () => ({
    items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "user", body: "the history", created_at: "2026-08-30T12:00:00Z" } }],
    has_more: false,
    thread_total: 1,
    thread_last_sequence: 1,
    sessions: [],
  });

  it("stands the strip and the saved conversation up with the machine saying nothing", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
      { items: historyThread().items, olderItemsRemain: false, deliveredSequence: 1, knownTotalItems: 1 },
    );
    feedSnapshot = { items: [{ ...feedItems[0], agents: [agent()] }], projects: [] };
    // Nothing this rail needs is on the wire: every read it makes is answered
    // by a machine that never replies.
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      return new Promise(() => {});
    });
    await mount();
    await flush();
    expect(bubbles().length).toBeGreaterThan(0);
    expect(railHost().querySelector("#rail-body").textContent).toContain("the history");
  });

  // The deployed bug: the rail remembers which agent was open across remounts,
  // and the revisit blanked the conversation the disk was already holding
  // until a read came back. There is no read to wait for now, and a row
  // arriving that says nothing about the conversation must not take it away.
  it("paints the saved history at once, and a row that says nothing does not blank it", async () => {
    feedSnapshot = { items: feedItems, projects: [] };
    payload = branchRow({ run: { run_id: "run-3", thread: historyThread() } });
    await mount();
    await flush();
    rail.dispose();
    rail = null;

    await mount();
    await flush();
    await pushRow(branchRow({ agents: [agent({ unread_count: 1 })] }));

    const body = railHost().querySelector("#rail-body");
    expect(body.textContent).toContain("the history");
    expect(body.textContent).not.toContain("No conversation yet");
  });
});

describe("the agent's surfaces, carried by the status row", () => {
  const shellSurfaces = {
    shells: [{ id: "sh-1", description: "cargo test", state: "running", tail: ["running 12 tests"] }],
  };

  const openPanelWithSurfaces = async () => {
    payload = branchRow({ agents: [agent({ surfaces: shellSurfaces })] });
    await mount();
  };

  it("mounts the pills in the scroller between the lead and the git facts", async () => {
    await openPanelWithSurfaces();

    const block = railHost().querySelector(".rail-composer");
    expect([...block.children].map((child) => child.id)).toEqual([
      "rail-surfaces-viewer",
      "rail-observation",
      // No issues host: what this agent is carrying on the board is a surface
      // now (#34), drawn behind a pill in the status row like every other
      // kind rather than as a block of its own above it.
      "rail-status",
      "rail-chat-recovery",
      "",
    ]);
    expect(block.querySelector('#rail-status-pills [data-surface-kind="shells"]')).not.toBe(null);
    expect(block.lastElementChild.querySelector("#railinput")).not.toBe(null);
  });

  it("opens a viewer with no row menu, leaving the draft and the focus alone", async () => {
    await openPanelWithSurfaces();
    const input = railHost().querySelector("#railinput");
    input.value = "half a sentence";
    input.focus();

    railHost().querySelector('[data-surface-kind="shells"]').click();
    await flush();

    const row = railHost().querySelector(".surface-shells .surface-row");
    expect(row.querySelector(".splitbtn")).toBe(null);
    expect(callsTo("thread.post")).toEqual([]);
    expect(railHost().querySelector("#railinput").value).toBe("half a sentence");
    expect(document.activeElement).toBe(input);
  });

  it("says so when the call a subagent row points at is outside the loaded conversation", async () => {
    payload = branchRow({
      agents: [
        agent({
          surfaces: {
            subagents: [{ id: "s1", label: "parser reviewer", state: "running", call_sequence: 9999 }],
          },
        }),
      ],
    });
    await mount();

    railHost().querySelector('[data-surface-kind="subagents"]').click();
    railHost().querySelector(".surface-subagents [data-call-sequence]").click();
    await flush();

    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError.mock.calls[0][0]).toContain("not in the loaded conversation");
  });
});

describe("the agent's surfaces, seeded from the local cache", () => {
  const generation = "surface-session-1";
  const feedItems = [{
    kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3", deviceId: "dev-1",
    agents: [agent(), agent({ id: "ag-2", ordinal: 2 })],
  }];
  const railContext = () => railAddress();
  const shellsRunning = (...descriptions) => ({
    shells: descriptions.map((description, index) => ({ id: `sh-${index}`, description, state: "running", tail: [] })),
  });
  const aChecklist = { checklist: [{ id: "t-1", subject: "wire the seed", state: "in_progress" }] };
  const surfacesAddress = (sub) => surfacesCacheAddress({ deviceId: "dev-1", entityId: "run-3", agentId: sub });
  const saveSurfaces = (sub, surfaces) => writeCached(surfacesAddress(sub), surfacesRecord(surfaces, generation));
  const saveSurfacesLongAgo = async (sub, surfaces) => {
    const clock = vi.spyOn(Date, "now").mockReturnValue(Date.now() - SURFACE_PILL_GRACE_MS - 1);
    await saveSurfaces(sub, surfaces);
    clock.mockRestore();
  };
  const savedSurfaces = (sub) => readCached(surfacesAddress(sub));
  const savedDescription = async (sub) => (await savedSurfaces(sub)).value.surfaces.shells[0].description;
  /// The SURFACE rows of the conversation's ⋮, which also carries the detail
  /// levels the thread is read at (core/conversationDetail.js).
  const menuKinds = () =>
    [...railHost().querySelectorAll(".rail-surface-menu .mi")]
      .map((item) => item.dataset.action)
      .filter((action) => !action.startsWith("detail:"));
  const pillKinds = () =>
    [...railStatusPills().querySelectorAll(".surface-pill")].map((pill) => pill.dataset.surfaceKind);
  const pillCount = (kind) =>
    railStatusPills().querySelector(`[data-surface-kind="${kind}"] .surface-pill-count`).textContent.trim();
  const openTasks = () => railStatusPills().querySelector('[data-surface-kind="checklist"]').click();
  const answerNothing = () => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      return {};
    });
  };

  beforeEach(() => {
    feedSnapshot = { items: feedItems, projects: [] };
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
  });

  it("paints the saved pills before the first read answers", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual(["shells"]);
  });

  it("paints no pill for an agent the cache never saw", async () => {
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual([]);
  });

  it("seeds no kind a live tab answers for from a snapshot older than the grace", async () => {
    await saveSurfacesLongAgo("ag-1", {
      ...shellsRunning("cargo test"),
      ...aChecklist,
      subagents: [{ id: "s-1", label: "reviewer", state: "running" }],
    });
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual(["checklist"]);
    openTasks();
    expect(railHost().querySelector(".surface-checklist").textContent).toContain("wire the seed");
  });

  it("seeds the same snapshot whole while the grace still holds", async () => {
    await saveSurfaces("ag-1", { ...shellsRunning("cargo test"), ...aChecklist });
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual(["shells", "checklist"]);
    openTasks();
    expect(railHost().querySelector(".surface-checklist-context").textContent).toContain("Last known");
  });

  it("offers the seeded kinds in the header menu before the first read answers", async () => {
    await saveSurfaces("ag-1", { ...shellsRunning("cargo test"), ...aChecklist });
    answerNothing();
    await mount();
    expect(menuKinds()).toEqual(["shells", "checklist"]);
  });

  it("opens the remembered kind's viewer on the saved snapshot", async () => {
    writeOpenSurface("run-3:ag-1", "shells");
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    await mount();
    expect(railHost().querySelector("#rail-surfaces-viewer").textContent).toContain("cargo test");
  });

  const shapelessRecords = [{ surfaces: "boom" }, { surfaces: { shells: "boom" } }, {}];
  for (const shapeless of shapelessRecords) {
    it(`paints the conversation and no pill for a record holding ${JSON.stringify(shapeless)}`, async () => {
      await writeCached(surfacesAddress("ag-1"), shapeless);
      await writeCached(
        { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
        {
          items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "user", body: "the history", created_at: "2026-08-30T12:00:00Z" } }],
          olderItemsRemain: false,
          deliveredSequence: 1,
          knownTotalItems: 1,
        },
      );
      answerNothing();
      await mount();
      expect(pillKinds()).toEqual([]);
      expect(menuKinds()).toEqual([]);
      expect(railHost().querySelector("#rail-body").textContent).toContain("the history");
      expect(notifyError).not.toHaveBeenCalled();
    });
  }

  it("seeds the agent switched to, not the one left behind", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    await saveSurfaces("ag-2", aChecklist);
    answerNothing();
    await mount();
    bubbles()[1].click();
    await flush();
    expect(pillKinds()).toEqual(["checklist"]);
    openTasks();
    expect(railHost().querySelector(".surface-checklist").textContent).toContain("wire the seed");
  });

  it("drops a seed whose agent was left while the read was in flight", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    rail = mountAgentRail(railHost(), railContext());
    bubbles()[1].click(); // ag-1's seed is still in flight
    await flush();
    expect(pillKinds()).toEqual([]);
  });

  // A snapshot is the row's to carry while the session that observed it is
  // alive, and the record's once it is not. Which of the two the panel paints
  // is decided by the generation: a restarted process inherits nothing.
  it("drops a mounted cached observation when a row arrives on another generation", async () => {
    await saveSurfaces("ag-1", {
      checklist: [{ id: "t-1", subject: "old process step", state: "in_progress" }],
      observations: { checklist: { support: "supported", freshness: "current", coverage: "complete" } },
    });
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual(["checklist"]);
    openTasks();
    expect(railHost().querySelector(".surface-checklist").textContent).toContain("old process step");

    await pushRow(branchRow({
      agents: [agent({ surface_session_generation: "surface-session-2", surfaces: null })],
    }));

    expect(railHost().querySelector(".agent-observation-host").hidden).toBe(true);
    expect(pillKinds()).toEqual([]);
    expect(railHost().textContent).not.toContain("old process step");
  });

  it("replaces the seeded pills with what the row's agent is observing now", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test", "cargo clippy"));
    answerNothing();
    await mount();
    expect(pillCount("shells")).toBe("2");

    await pushRow(branchRow({ agents: [agent({ surfaces: shellsRunning("cargo test") })] }));

    expect(pillCount("shells")).toBe("1");
  });

  it("leaves no seed landing after the rail is gone", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    rail = mountAgentRail(railHost(), railContext());
    rail.dispose();
    rail = null;
    await flush();
    expect(railHost().querySelector(".surface-pill")).toBe(null);
    expect(notifyError).not.toHaveBeenCalled();
  });
});

describe("task completion notifications", () => {
  const checklistAgent = (state, epoch) => agent({
    surfaces: {
      checklist: [{ id: "turn-1:0", subject: "Ship the release", state }],
      observations: { checklist: { support: "supported", freshness: "current", coverage: "complete" } },
      checklist_provenance: {
        source: "turn_plan", provider_session_generation: 1, turn_id: "turn-1",
        collection_epoch: epoch, carried_from_prior_turn: false,
      },
    },
  });

  it("announces a live transition once and removes the toast with the rail", async () => {
    payload = branchRow({ agents: [checklistAgent("in_progress", 1)] });
    await mount();
    bubbles()[0].getBoundingClientRect = () => ({ left: 900, right: 932, top: 100, bottom: 132, width: 32, height: 32 });
    expect(document.querySelector(".task-completion-toast")).toBe(null);

    payload = branchRow({ agents: [checklistAgent("completed", 2)] });
    await pushRow();
    expect(document.querySelector(".task-completion-toast")?.textContent).toContain("Ship the release");

    await pushRow();
    expect(document.querySelectorAll(".task-completion-toast")).toHaveLength(1);
    rail.dispose();
    rail = null;
    expect(document.querySelector(".task-completion-toast")).toBe(null);
  });
});

// The bridge behind these is the machine's own, handed over mid-test: a
// workspace whose checkout this bridge holds no run for answers differently,
// it is not another machine.
describe("a workspace's conversation", () => {
  // A workspace is not addressed by its own id: the conversation it holds is
  // its entity, and the two lists are what say which. The rail finds its row
  // the way the sync layer finds the workspace to watch.
  it("finds the row the workspace's conversation is on, and posts through it", async () => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "thread.post") return {
        entity_id: "run-3", agent_id: "ag-workspace", conversation_id: "conversation-workspace", posted_sequence: 7,
      };
      return {};
    });
    payload = branchRow({
      agents: [agent({ id: "ag-workspace", conversation_id: "conversation-workspace", state: "live" })],
    });
    await writeRailBoard({
      items: [payload],
      projects: [{ project_id: "p1", name: "build" }],
      workspaces: [{ id: "ws-1", project_id: "p1", name: "login", status: "ready", entity_id: "run-3" }],
    });

    await mount({ kind: "workspace", projectId: "p1", workspaceId: "ws-1", sourceId: "root" });
    const input = railHost().querySelector("#railinput");
    expect(input).not.toBeNull();
    input.value = "fix the deployed workspace";
    railHost().querySelector("#railsend").click();
    await flush();

    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3",
      agent_id: "ag-workspace",
      conversation_id: "conversation-workspace",
      body: "fix the deployed workspace",
    });
  });

  it("shows the shared new-conversation composer without creating storage on open", async () => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return { workspace: { id: "workspace-1", project_id: "p1", entity_id: null, agents: [] } };
      return {};
    });

    await mount({ kind: "workspace", projectId: "p1", workspaceId: "workspace-1" });
    if (!panel()) {
      railHost().querySelector('[data-bubble="ghost"]').click();
      await flush();
    }

    expect(railHost().querySelector(".rail-newagent")).not.toBeNull();
    expect(railHost().querySelector("#railinput")).not.toBeNull();
    expect(callsTo("workspace.ensure_conversation")).toEqual([]);
    expect(callsTo("agent.add")).toEqual([]);
  });

  it("creates the workspace conversation on first send and uses saved defaults", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "claude_adk", model: "claude-opus-5", effort: "high",
    }));
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return { workspace: { id: "workspace-1", project_id: "p1", entity_id: null, agents: [] } };
      if (method === "workspace.ensure_conversation") return { workspace_id: "workspace-1", entity_id: "run-workspace" };
      if (method === "agent.add") return { entity_id: "run-workspace", agent: agent({ id: "ag-workspace", ordinal: 1, state: "idle" }) };
      if (method === "thread.post") return { posted_sequence: 1 };
      if (method === "agent.start") return { agent_id: "ag-workspace" };
      return {};
    });

    await mount({ kind: "workspace", projectId: "p1", workspaceId: "workspace-1", autofocusComposer: true });
    railHost().querySelector("#railinput").value = "start in this workspace";
    railHost().querySelector("#railsend").click();
    await flush();

    expect(calls.filter((entry) => ["workspace.ensure_conversation", "agent.add", "thread.post", "agent.start"].includes(entry.method))
      .map((entry) => entry.method)).toEqual(["workspace.ensure_conversation", "agent.add", "thread.post", "agent.start"]);
    expect(callsTo("workspace.ensure_conversation")[0].params).toEqual({ workspace_id: "workspace-1" });
    expect(callsTo("agent.add")[0].params).toMatchObject({
      entity_id: "run-workspace", provider: "claude_adk", model: "claude-opus-5", effort: "high",
    });
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-workspace", agent_id: "ag-workspace", body: "start in this workspace",
    });
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-workspace", agent_id: "ag-workspace" });
  });

  it("restores the draft and retries workspace creation after ensure fails", async () => {
    let ensureAttempts = 0;
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return { workspace: { id: "workspace-1", project_id: "p1", entity_id: null, agents: [] } };
      if (method === "workspace.ensure_conversation") {
        ensureAttempts += 1;
        if (ensureAttempts === 1) throw new Error("workspace unavailable");
        return { entity_id: "run-workspace" };
      }
      if (method === "agent.add") return { agent: agent({ id: "ag-workspace", state: "idle" }) };
      return {};
    });

    await mount({ kind: "workspace", projectId: "p1", workspaceId: "workspace-1", autofocusComposer: true });
    railHost().querySelector("#railinput").value = "keep this draft";
    railHost().querySelector("#railsend").click();
    await flush();
    expect(railHost().querySelector("#railinput").value).toBe("keep this draft");
    expect(railHost().querySelector(".rail-newagent")).not.toBeNull();
    expect(callsTo("agent.add")).toEqual([]);
    expect(callsTo("thread.post")).toEqual([]);
    expect(callsTo("agent.start")).toEqual([]);
    expect(notifyError).toHaveBeenCalledWith("Could not start the agent", "workspace unavailable");

    railHost().querySelector("#railsend").click();
    await flush();
    expect(callsTo("workspace.ensure_conversation")).toHaveLength(2);
    expect(callsTo("agent.add")).toHaveLength(1);
  });
});

describe("creating an agent, before the daemon has answered for it", () => {
  const agentless = () => branchRow({ agents: [] });
  const composer = () => railHost().querySelector("#railinput");
  const timeline = () => railHost().querySelector(".thread-items");

  const holdAgentAdd = () => {
    let release = null;
    const held = new Promise((resolve) => {
      release = resolve;
    });
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "agent.add") return answer(method, params);
      calls.push({ method, params });
      await held;
      return { entity_id: "run-3", agent: agent({ id: "ag-2", ordinal: 2, state: "idle" }) };
    });
    return release;
  };

  const refuseCall = (refused, message) => {
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== refused) return answer(method, params);
      calls.push({ method, params });
      throw new Error(message);
    });
  };

  const press = async (body) => {
    composer().value = body;
    railHost().querySelector("#railsend").click();
    await flush();
  };

  it("paints a pending agent the daemon has not answered for yet", async () => {
    payload = agentless();
    await mount();
    let release = null;
    const held = new Promise((resolve) => {
      release = resolve;
    });
    const running = chatRepository.optimisticStore().runOptimistic({
      scope: `${chatRepository.scopeKey}:agents:branch:p1:build/login`,
      records: [insertRecord("ag-7", agent({ id: "ag-7", ordinal: 1 }))],
      call: () => held,
      failureSummary: "Could not start the agent",
    });
    await flush();

    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-7", ""]);

    vi.advanceTimersByTime(3200);
    await flush();
    expect(bubbles().map((bubble) => bubble.dataset.agent)).toEqual(["ag-7", ""]);

    release();
    await running;
  });

  it("paints the agent, its conversation and a cleared box in the same tick as the press", async () => {
    payload = agentless();
    await mount();
    const release = holdAgentAdd();

    await press("start here");

    expect(bubbles().map((bubble) => bubble.dataset.bubble)).toEqual(["agent", "add"]);
    expect(bubbles()[0].classList.contains("active")).toBe(true);
    // The agent the press made has named nothing yet: the head says its harness.
    expect(headWho(panel())).toBe("Claude Code");
    expect(timeline().textContent).toContain("start here");
    expect(composer().value).toBe("");
    expect(callsTo("agent.add")).toHaveLength(1);
    expect(callsTo("thread.post")).toEqual([]);

    release();
    await flush();
  });

  it("renames the bubble to the agent the daemon made, without rebuilding anything", async () => {
    payload = agentless();
    await mount();
    const release = holdAgentAdd();
    await press("start here");
    const bubble = bubbles()[0];
    const painter = livePainters().at(-1);
    const body = railHost().querySelector("#rail-body");
    const input = composer();
    input.focus();

    release();
    await flush();

    expect(bubbles()[0]).toBe(bubble);
    expect(bubble.dataset.agent).toBe("ag-2");
    expect(painter.destroyed).toBe(false);
    expect(railHost().querySelector("#rail-body")).toBe(body);
    expect(composer()).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3", agent_id: "ag-2", body: "start here",
    });
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-3", agent_id: "ag-2" });
  });

  it("keeps the sent message on screen while the post behind it is still in flight", async () => {
    payload = agentless();
    await mount();
    let releasePost = null;
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      await new Promise((resolve) => {
        releasePost = resolve;
      });
      return { posted_sequence: 7 };
    });

    await press("start here");

    expect(bubbles()[0].dataset.agent).toBe("ag-2");
    expect(timeline().textContent).toContain("start here");

    releasePost();
    await flush();
    expect(timeline().textContent).toContain("start here");
  });

  it("keeps the bubble it painted when the read that names the real agent lands", async () => {
    payload = agentless();
    await mount();
    await press("start here");
    const bubble = bubbles()[0];
    const painter = livePainters().at(-1);
    bubble.focus();
    payload = branchRow({ agents: [agent({ id: "ag-2", ordinal: 2, state: "idle" })] });

    await pushRow();

    expect(bubbles()[0]).toBe(bubble);
    expect(painter.destroyed).toBe(false);
    expect(document.activeElement).toBe(bubble);
  });

  it("leaves the optimistic agent standing when a row lands mid-flight", async () => {
    payload = agentless();
    await mount();
    const release = holdAgentAdd();
    await press("start here");
    const bubble = bubbles()[0];

    // The board has not heard of the agent this tab is creating; the bubble
    // the reader is typing into is not taken off them for that.
    await pushRow(agentless());

    expect(bubbles()[0]).toBe(bubble);
    expect(bubbles()).toHaveLength(2);

    release();
    await flush();
  });

  it("puts the message back in the box when the agent could not be created", async () => {
    payload = agentless();
    await mount();
    refuseCall("agent.add", "no room");

    await press("start here");

    expect(bubbles().map((bubble) => bubble.dataset.bubble)).toEqual(["ghost"]);
    expect(railHost().querySelector(".rail-newagent")).toBeTruthy();
    expect(composer().value).toBe("start here");
    expect(document.activeElement).not.toBe(composer());
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("Could not start the agent", "no room");
  });

  it("keeps the agent when only the message was refused", async () => {
    payload = agentless();
    await mount();
    refuseCall("thread.post", "no conversation");

    await press("start here");

    expect(bubbles().map((bubble) => bubble.dataset.bubble)).toEqual(["agent", "add"]);
    expect(bubbles()[0].dataset.agent).toBe("ag-2");
    expect(timeline().textContent).not.toContain("start here");
    expect(composer().value).toBe("start here");
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("Could not start the agent", "no conversation");
  });

  it("posts a message typed before the agent existed to the agent that now does", async () => {
    payload = agentless();
    await mount();
    const release = holdAgentAdd();
    await press("start here");
    await press("and this too");

    release();
    await flush();
    await flush();

    const posts = callsTo("thread.post");
    expect(posts.map((call) => call.params.body)).toEqual(["start here", "and this too"]);
    expect(posts.every((call) => call.params.agent_id === "ag-2")).toBe(true);
  });
});

describe("sending to an agent that is already there", () => {
  const composer = () => railHost().querySelector("#railinput");
  const timeline = () => railHost().querySelector(".thread-items");
  const copiesOf = (body) => (timeline() ? timeline().textContent.split(body).length - 1 : 0);

  const holdThreadPost = () => {
    let release = null;
    const held = new Promise((resolve) => {
      release = resolve;
    });
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      await held;
      return { posted_sequence: 7 };
    });
    return release;
  };

  const refuseThreadPost = (message) => {
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      throw new Error(message);
    });
  };

  const press = async (body) => {
    composer().value = body;
    railHost().querySelector("#railsend").click();
    await flush();
  };

  // Issue #58, through the rail as a surface mounts it: the device's limit is a
  // strip at the top of the conversation, the message sent meanwhile is Queued
  // with the same reason on hover, and both go when the bridge clears it.
  it("says a harness is out of usage over the conversation, and why a message is still queued", async () => {
    resetUsageLimits();
    const resetsAt = new Date(Date.now() + 34 * 60_000).toISOString();
    setUsageLimits("dev-1", [{
      harness: "claude_adk",
      since: new Date().toISOString(),
      resets_at: resetsAt,
      said: "You've hit your session limit · resets 6:20pm (America/New_York)",
    }]);
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();

    const banner = panel().querySelector(".usage-limit-banner");
    expect(banner.textContent).toContain("Claude session limit reached · resets in 34 min");
    expect(banner.querySelector(".usage-limit-said").textContent).toContain("You've hit your session limit");
    expect(composer().disabled).toBe(false);

    const release = holdThreadPost();
    await press("are you still on this?");
    const queued = timeline().querySelector('[data-delivery-status="queued"]');
    expect(queued.title).toBe("Claude session limit reached · resets in 34 min");

    setUsageLimits("dev-1", []);
    expect(panel().querySelector(".usage-limit-banner")).toBeNull();
    expect(timeline().querySelector('[data-delivery-status="queued"]').hasAttribute("title")).toBe(false);
    release();
    await flush();
  });

  it("shows the message and clears the box before thread.post answers", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    const release = holdThreadPost();

    await press("look at the login flow");

    expect(timeline().textContent).toContain("look at the login flow");
    expect(timeline().querySelector('[data-delivery-status="queued"]').textContent).toBe("Queued");
    expect(composer().value).toBe("");
    expect(railHost().querySelector("#railsend").disabled).toBe(false);
    expect(callsTo("thread.post")).toHaveLength(1);

    release();
    await flush();
  });

  it("keeps the sent message standing through a read that does not carry it yet", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    const release = holdThreadPost();
    await press("look at the login flow");

    await pushRow();

    expect(copiesOf("look at the login flow")).toBe(1);

    release();
    await flush();
  });

  it("rekeys the sent message onto the sequence thread.post names, without a second copy", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    await press("look at the login flow");
    expect(copiesOf("look at the login flow")).toBe(1);

    payload = branchRow({
      agents: [agent({ state: "live" })],
      run: {
        run_id: "run-3",
        thread: {
          items: [{ type: "message", data: { sequence: 7, role: "user", body: "look at the login flow" } }],
          sessions: [],
        },
      },
    });
    await pushRow();

    expect(copiesOf("look at the login flow")).toBe(1);
  });

  it("leaves a delivered message where it landed when only the wake is refused", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "agent.start") return answer(method, params);
      calls.push({ method, params });
      throw new Error("no session could be spawned");
    });

    await press("look at the login flow");

    expect(callsTo("thread.post")).toHaveLength(1);
    expect(copiesOf("look at the login flow")).toBe(1);
    expect(composer().value).toBe("");
    expect(notifyError).toHaveBeenCalledTimes(1);
  });

  // The post landed. A start behind it that outlives the timer says nothing
  // about the post, so raising "Message failed" would be an error about a
  // message that is on the thread.
  it("keeps a delivered message, and raises nothing, when only the wake outlives the timer", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "agent.start") return answer(method, params);
      calls.push({ method, params });
      const timedOut = new Error("agent.start timed out");
      timedOut.timedOut = true;
      timedOut.uncertain = true;
      throw timedOut;
    });

    await press("look at the login flow");

    expect(callsTo("thread.post")).toHaveLength(1);
    expect(copiesOf("look at the login flow")).toBe(1);
    expect(composer().value).toBe("");
    expect(notifyError).not.toHaveBeenCalled();
  });

  // The turn is durable the moment the daemon takes it; the answer can outlive
  // the browser's timer behind a cold spawn. Handing the draft back would have
  // the human send the same turn twice and the agent hear it twice.
  it("keeps the message on the thread when the post itself outlives the timer", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      const timedOut = new Error("thread.post timed out");
      timedOut.timedOut = true;
      timedOut.uncertain = true;
      throw timedOut;
    });

    await press("look at the login flow");

    expect(copiesOf("look at the login flow")).toBe(1);
    expect(composer().value).toBe("");
    expect(notifyError).not.toHaveBeenCalled();
  });

  // ...and the conversation carrying it is what takes the stand-in away. The
  // item names the operation that made it (bridge `ThreadMessage.operation_id`,
  // on the page and on the push alike), which is the only thing that can say
  // so: the browser never heard a sequence to wait under.
  it("takes the stand-in away when the conversation carries a timed-out post's message", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    const answer = bridge.call;
    bridge.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      const timedOut = new Error("thread.post timed out");
      timedOut.timedOut = true;
      timedOut.uncertain = true;
      throw timedOut;
    });

    await press("look at the login flow");
    expect(copiesOf("look at the login flow")).toBe(1);

    const operationId = callsTo("thread.post")[0].params.operation_id;
    expect(operationId).toBeTruthy();
    await pushRailThreadItems("run-3", "ag-1", [{
      type: "message",
      data: { sequence: 7, role: "user", body: "look at the login flow", operation_id: operationId },
    }]);
    await flush();

    expect(copiesOf("look at the login flow")).toBe(1);
  });

  it("puts the words back in the box and says why when thread.post is refused", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    refuseThreadPost("the conversation is gone");

    await press("look at the login flow");

    expect(copiesOf("look at the login flow")).toBe(0);
    expect(composer().value).toBe("look at the login flow");
    expect(document.activeElement).not.toBe(composer());
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("Message failed", "the conversation is gone");
  });
});

describe("the one status row", () => {
  const checklistSurfaces = (state = "in_progress") => ({
    checklist: [{ id: "c1", subject: "Land the fold", state }],
  });

  const aTurnInFlight = () =>
    (payload = branchRow({
      ...payload,
      agents: [agent({ ...payload.agents?.[0], working_time: { since: new Date(Date.now() - 85000).toISOString(), seconds: 85 } })],
    }), pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login", deviceId: "dev-1",
        working: true, working_time: { since: new Date(Date.now() - 85000).toISOString(), seconds: 85 },
        stat: { insertions: 4, deletions: 1, ahead: 2, behind: 0 },
      }],
      projects: [],
    }));

  it("lays the row out lead, pills, git — and wears no dot anywhere", async () => {
    payload = branchRow({ agents: [agent({ surfaces: checklistSurfaces() })] });
    await aTurnInFlight();
    await mount();

    expect([...railStatus().children].map((child) => child.id)).toEqual([
      "rail-status-lead",
      "rail-status-pills",
      "rail-status-git",
    ]);
    expect(railStatusPills().querySelector('[data-surface-kind="checklist"]')).toBeTruthy();
    expect(railHost().querySelector(".sdot")).toBe(null);
  });

  it("spans the whole width on a phone, so nothing shows beside the conversation", () => {
    const phoneRule = shellCss.match(/@media \(max-width: 760px\) \{[\s\S]*?\.rail-panel \{([^}]*)\}/);
    expect(phoneRule).not.toBe(null);
    expect(phoneRule[1]).toContain("left:0");
    // The strip runs across the column's foot at this width, not down its
    // edge, so the panel takes the whole width and stops on it from above.
    expect(phoneRule[1]).toContain("right:0");
    expect(phoneRule[1]).toContain("bottom:calc(var(--console-space) + var(--agent-strip))");
    expect(phoneRule[1]).toContain("width:auto");
    expect(phoneRule[1]).not.toContain("100vw");
  });

  it("keeps the clock's digits fixed in width so a tick never nudges the pills", () => {
    const clockRule = shellCss.match(/\.rail-status-text \{[^}]*\}/);
    expect(clockRule).not.toBe(null);
    expect(clockRule[0]).toContain("font-variant-numeric:tabular-nums");
    expect(clockRule[0]).toContain("min-width:5ch");
    expect(clockRule[0]).toContain("display:inline-block");
  });

  // The head of a conversation whose agent has not named its work yet is
  // mid-something too, so it wears the same shimmer as the clocks rather than
  // a copy of it — one rule, one keyframe, one reduced-motion answer.
  it("shimmers the status clock, every ticking row clock and a starting head through one rule, holding still under reduced motion", () => {
    expect(shellCss.match(/@keyframes clock-shimmer/g)).toHaveLength(1);
    expect(shellCss.match(/linear-gradient\(100deg/g)).toHaveLength(1);
    const shimmering = /\.rail-status-working, \.surface-row-clock\[data-running-since\], \.rail-who-starting/;
    const shimmerRule = shellCss.match(new RegExp(`\\n${shimmering.source} \\{ color:transparent;[^}]*\\}`));
    expect(shimmerRule[0]).toContain("animation:clock-shimmer");
    expect(shimmerRule[0]).toContain("background-clip:text");
    expect(shimmerRule[0]).toContain("var(--clock-ink)");
    const stillRule = shellCss.match(
      new RegExp(`@media \\(prefers-reduced-motion: reduce\\) \\{\\n?\\s*${shimmering.source} \\{[^}]*\\}`),
    );
    expect(stillRule).not.toBe(null);
    expect(stillRule[0]).toContain("animation:none");
    // Each wearer brings its own ink: the working clock the accent, a starting
    // head the dim.
    expect(shellCss).toContain(".rail-who-starting { --clock-ink:var(--dim); font-weight:500; }");
  });

  it("wears the working colour on the status clock and grey on a row clock, holding its digits still", () => {
    expect(shellCss.match(/\.rail-status-working \{ --clock-ink:var\(--accent\); \}/)).not.toBe(null);
    const rowClockRule = shellCss.match(/\n\.surface-row-clock \{[^}]*\}/);
    expect(rowClockRule[0]).toContain("--clock-ink:var(--dim)");
    expect(rowClockRule[0]).toContain("font-variant-numeric:tabular-nums");
    expect(rowClockRule[0]).toContain("min-width:4ch");
  });

  it("reads Working and the clock while no pill is asking for the room", async () => {
    await aTurnInFlight();
    await mount();
    await motionSettled();

    expect(railStatusLead().textContent).toBe("Working 1:25");
    expect(workingWord().hidden).toBe(false);
  });

  it("collapses the word Working under a pill and grows it back when the last one goes", async () => {
    payload = branchRow({ agents: [agent({ surfaces: checklistSurfaces() })] });
    await aTurnInFlight();
    await mount();
    await motionSettled();

    expect(workingWord().hidden).toBe(true);
    expect(railStatusLead().textContent).toContain("1:25");

    payload = branchRow({ agents: [agent({ surfaces: {} })] });
    payload.agents[0].working_time = { since: new Date(Date.now() - 85000).toISOString(), seconds: 85 };
    await pushRow();
    await motionSettled();

    expect(railStatusPills().querySelector(".surface-pill")).toBe(null);
    expect(workingWord().hidden).toBe(false);
  });

  it("grows the row and its lead into place, and shrinks them out when the row falls quiet", async () => {
    const started = recordAnimations();
    const movesOn = (element) => started.filter((run) => run.element === element);
    try {
      await aTurnInFlight();
      await mount();
      await settleMotion();

      expect(railStatus().hidden).toBe(false);
      expect(movesOn(railStatus())[0].keyframes[0]).toEqual({ height: "0px", opacity: 0 });
      expect(movesOn(railStatusLead())[0].keyframes[0]).toEqual({ width: "0px", opacity: 0 });
      // The git facts are not a box that grows: their characters cascade in one
      // at a time, so nothing animates the group itself.
      expect(movesOn(railStatusGit())).toEqual([]);
      expect(railStatusGit().textContent).toBe("↑2+4−1");

      started.length = 0;
      payload = branchRow({ agents: [agent({ working_time: null })] });
      await pushRow();
      await pushFeed({ items: [], projects: [] });
      await settleMotion();

      expect(movesOn(railStatusLead())[0].keyframes[1]).toEqual({ width: "0px", opacity: 0 });
      expect(railStatusGit().textContent).toBe("");
      expect(movesOn(railStatus())[0].keyframes[1]).toEqual({ height: "0px", opacity: 0 });
      expect(railStatus().hidden).toBe(true);
    } finally {
      stopRecordingAnimations();
    }
  });

  it("scrolls the pills in the room between the lead and the git facts, with no bar to show for it", async () => {
    await mount();
    expect(railStatusPills().className).toContain("scrollstrip");
    const stripRule = shellCss.match(/\.scrollstrip \{[^}]*\}/)[0];
    expect(stripRule).toMatch(/overflow-x:auto/);
    expect(stripRule).toMatch(/scrollbar-width:none/);
    expect(shellCss).toMatch(/\.scrollstrip::-webkit-scrollbar \{[^}]*display:none/);
    expect(shellCss.match(/\.rail-status-pills \{[^}]*\}/)[0]).toMatch(/mask-image:linear-gradient/);
  });
});

describe("the viewer above the conversation footer", () => {
  it("is anchored inside the composer block so opening it cannot reflow the transcript", async () => {
    payload = branchRow({
      agents: [agent({ surfaces: { shells: [{ id: "sh-1", description: "cargo test", state: "running", tail: [] }] } })],
    });
    await mount();

    expect([...panel().children].map((child) => child.className)).toEqual([
      "rail-head",
      "rail-body",
      "rail-composer",
    ]);
    expect(panel().querySelector(".rail-composer > .rail-surfaces-viewer")).not.toBeNull();
    expect(railHost().querySelector("#rail-surfaces-viewer").hidden).toBe(true);
  });

  it("is an independently scrolling popover anchored above the footer", () => {
    const viewerRule = shellCss.match(/\.rail-surfaces-viewer \{[^}]*\}/)[0];
    const composerRule = shellCss.match(/\.rail-composer \{[^}]*\}/)[0];
    expect(composerRule).toMatch(/position:absolute/);
    expect(viewerRule).toMatch(/position:absolute/);
    expect(viewerRule).toMatch(/bottom:100%/);
    expect(viewerRule).toMatch(/max-height:min\(46vh, 420px\)/);
    expect(viewerRule).toMatch(/overflow:hidden/);
    expect(shellCss).toMatch(/\.surface-popover-body \{[^}]*overflow-y:auto/);
  });
});

// A folded run of activity is a head until the reader presses it, and what it
// opens onto is not always in hand: the daemon caps how much of one run a page
// carries and says in a digest how far the whole of it reaches.
describe("a run of activity in the rail", () => {
  const toolCall = (sequence, summary) => ({
    type: "event",
    data: { sequence, event: "tool_use", summary },
  });

  const said = (sequence, body) => ({ type: "message", data: { sequence, id: `m-${sequence}`, role: "agent", body } });

  const conversation = (items, digests) => branchRow({
    run: {
      run_id: "run-3",
      thread: {
        sessions: [],
        items,
        has_more: false,
        thread_total: items.length,
        thread_last_sequence: items[items.length - 1].data.sequence,
        activity_digests: digests,
      },
    },
  });

  const runHead = () => railHost().querySelector(".thread-activity-group-head");
  const runBox = () => railHost().querySelector("details.thread-activity-group");
  const runRows = () => [...railHost().querySelectorAll(".thread-activity-group-list > .thread-activity")];

  const answering = (activityPage) => {
    bridge.call.mockImplementation(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return payload;
      if (method === "thread.activity") return activityPage;
      return {};
    });
  };

  it("draws a shut run as a head, and its rows on the press that opens it", async () => {
    payload = conversation([said(1, "Have a look."), toolCall(2, "Read a.js"), toolCall(3, "Read b.js")], [
      { from_sequence: 2, through_sequence: 3, tool_calls: 2, rows: 2, last_tool_call: null },
    ]);
    await mount();

    expect(railHost().querySelector(".thread-activity-count").textContent).toBe("2");
    expect(runRows()).toHaveLength(0);
    expect(runBox().open).toBe(false);

    runHead().click();
    await flush();

    expect(runRows()).toHaveLength(2);
    expect(runBox().open).toBe(true);
    expect(callsTo("thread.activity")).toEqual([]);

    runHead().click();
    await flush();

    expect(runRows()).toHaveLength(0);
    expect(runBox().open).toBe(false);
  });

  it("asks for the half of a cut run the window never held, once", async () => {
    payload = conversation(
      [said(1, "Have a look."), toolCall(50, "Read y.js"), toolCall(51, "Read z.js"), said(52, "Done.")],
      [{ from_sequence: 10, through_sequence: 51, tool_calls: 40, rows: 40, last_tool_call: null }],
    );
    answering({ items: [toolCall(10, "Read a.js")], oldest_sequence: 10, has_more: false });
    await mount();

    runHead().click();
    await flush();

    expect(callsTo("thread.activity").map((call) => call.params)).toEqual([{
      entity_id: "run-3",
      agent_id: "ag-1",
      from_sequence: 10,
      through_sequence: 51,
      limit: 200,
    }]);
    expect(runRows().map((row) => row.dataset.sequence)).toEqual(["10", "50", "51"]);

    runHead().click();
    runHead().click();
    await flush();

    expect(callsTo("thread.activity")).toHaveLength(1);
  });

  // The tail run is the one still being written, and a record is kept until the
  // entity is evicted — so freezing a live run into one would hide every call it
  // grew afterwards. The window is where the tail's rows land instead.
  it("never asks for the run that reaches the end of the conversation", async () => {
    payload = conversation([said(1, "Have a look."), toolCall(50, "Read y.js"), toolCall(51, "Read z.js")], [
      { from_sequence: 10, through_sequence: 51, tool_calls: 40, rows: 40, last_tool_call: null },
    ]);
    answering({ items: [toolCall(10, "Read a.js")], oldest_sequence: 10, has_more: false });
    await mount();

    runHead().click();
    await flush();

    expect(callsTo("thread.activity")).toEqual([]);
    expect(runRows().map((row) => row.dataset.sequence)).toEqual(["50", "51"]);
  });

  // The digest that says how far a run reaches is cut on a PAGED answer, and no
  // forward delta refreshes it — while the newest sequence of the conversation
  // moves on every delta. So one tool call landing on the live tail run leaves
  // the digest behind the end of the conversation, and the digest alone would
  // then call a run that is still being written historical.
  it("never asks for a tail run one delta has grown past its digest", async () => {
    payload = conversation([said(1, "Have a look."), toolCall(50, "Read y.js"), toolCall(51, "Read z.js")], [
      { from_sequence: 10, through_sequence: 51, tool_calls: 40, rows: 40, last_tool_call: null },
    ]);
    answering({ items: [toolCall(10, "Read a.js")], oldest_sequence: 10, has_more: false });
    await mount();

    await pushRailThreadItems("run-3", "ag-1", [toolCall(52, "Read q.js")]);
    await flush();

    runHead().click();
    await flush();

    expect(callsTo("thread.activity")).toEqual([]);
    expect(
      await readCached({ deviceId: "dev-1", entityId: "run-3", kind: ACTIVITY_RECORD_KIND, sub: "ag-1:10" }),
    ).toBeUndefined();
    expect(runRows().map((row) => row.dataset.sequence)).toEqual(["50", "51", "52"]);
  });

  // What to fetch is decided over the digests the TIMELINE was painted from,
  // not over whatever the window holds at the moment of the press: the two part
  // company for a frame whenever the record moves under an open panel, and
  // reading the window instead leaves that press asking for nothing.
  it("asks over the digests the timeline was painted from, not the window's", async () => {
    payload = conversation(
      [said(1, "Have a look."), toolCall(50, "Read y.js"), toolCall(51, "Read z.js"), said(60, "Done.")],
      [{ from_sequence: 10, through_sequence: 51, tool_calls: 40, rows: 40, last_tool_call: null }],
    );
    answering({ items: [toolCall(10, "Read a.js")], oldest_sequence: 10, has_more: false });
    await mount();

    // A row arrives saying nothing about the conversation; the digests the
    // timeline was drawn with are still what the press is answered over.
    await pushRow(branchRow({ agents: [agent({ unread_count: 1 })] }));

    runHead().click();
    await flush();

    expect(callsTo("thread.activity").map((call) => call.params.from_sequence)).toEqual([10]);
  });

  it("opens the run a surface's call is folded into before reaching for the row", async () => {
    payload = conversation([said(1, "Have a look."), toolCall(2, "Task(review the parser)")], [
      { from_sequence: 2, through_sequence: 2, tool_calls: 1, rows: 1, last_tool_call: null },
    ]);
    payload.agents = [agent({ surfaces: { subagents: [{ id: "s1", label: "parser reviewer", state: "running", call_sequence: 2 }] } })];
    await mount();

    railHost().querySelector('[data-surface-kind="subagents"]').click();
    railHost().querySelector(".surface-subagents [data-call-sequence]").click();
    await flush();

    expect(notifyError).not.toHaveBeenCalled();
    expect(railHost().querySelector('[data-sequence="2"]')).not.toBe(null);
  });

  // The reader pressed a fold and the daemon could not answer: the box still
  // opens onto the rows the window holds, and what did not arrive is said out
  // loud rather than left as an empty box.
  it("says so when the half of a run it asked for does not arrive", async () => {
    payload = conversation(
      [said(1, "Have a look."), toolCall(50, "Read y.js"), said(52, "Done.")],
      [{ from_sequence: 10, through_sequence: 50, tool_calls: 40, rows: 40, last_tool_call: null }],
    );
    bridge.call.mockImplementation(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return payload;
      if (method === "thread.activity") throw new Error("entity is not loaded");
      return {};
    });
    await mount();

    runHead().click();
    await flush();

    expect(notifyError).toHaveBeenCalledWith("Could not load this activity", "entity is not loaded");
    expect(runRows().map((row) => row.dataset.sequence)).toEqual(["50"]);
  });

  // A run the page cut holds calls no row in the window stands for, and the
  // reference a surface carries can point at one of them. What says which run a
  // sequence belongs to is the digest's span, not the oldest row in hand.
  it("reaches a call in the half of a cut run the window never held", async () => {
    payload = conversation(
      [said(1, "Have a look."), toolCall(50, "Read z.js"), said(60, "Done.")],
      [{ from_sequence: 10, through_sequence: 50, tool_calls: 40, rows: 40, last_tool_call: null }],
    );
    payload.agents = [agent({ surfaces: { subagents: [{ id: "s1", label: "parser reviewer", state: "running", call_sequence: 12 }] } })];
    answering({
      items: [toolCall(12, "Task(review the parser)")],
      oldest_sequence: 12,
      has_more: false,
    });
    await mount();

    railHost().querySelector('[data-surface-kind="subagents"]').click();
    railHost().querySelector(".surface-subagents [data-call-sequence]").click();
    await flush();

    expect(notifyError).not.toHaveBeenCalled();
    expect(railHost().querySelector('[data-sequence="12"]')).not.toBe(null);
  });
});

// The conversation repaints on every poll and every event, and most of those
// ticks resolve exactly what the last one did.
describe("a chat paint with nothing to say", () => {
  const said = (sequence, body) => ({ type: "message", data: { sequence, id: `m-${sequence}`, role: "agent", body } });

  const conversationOf = (items) => branchRow({
    run: {
      run_id: "run-3",
      thread: {
        sessions: [],
        items,
        has_more: false,
        thread_total: items.length,
        thread_last_sequence: items[items.length - 1].data.sequence,
      },
    },
  });

  // A repaint writes every row it disagrees with, and a mark nobody rendered
  // is a disagreement — so a mark left on a row survives exactly the ticks the
  // paint skipped.
  const markTheFirstRow = () => railHost().querySelector(".thread-items").firstElementChild.setAttribute("data-probe", "1");
  const markSurvived = () => railHost().querySelector('[data-probe="1"]') !== null;

  it("builds nothing on a tick that moved none of its inputs", async () => {
    payload = conversationOf([said(1, "the first thing said")]);
    await mount();
    markTheFirstRow();

    await pushRow();

    expect(markSurvived()).toBe(true);
  });

  it("paints again the moment one of them does", async () => {
    payload = conversationOf([said(1, "the first thing said")]);
    await mount();
    markTheFirstRow();

    payload = conversationOf([said(1, "the first thing said"), said(2, "and the next")]);
    await pushRow();

    expect(markSurvived()).toBe(false);
    expect(railHost().querySelector(".thread-items").textContent).toContain("and the next");
  });
});

// The rail merges every device's rows, and every machine mints a `p1`. The work
// item the rail is standing on is on one machine — the home device's, until a
// route can name its own — so the row it seeds and reports from is that
// device's, not whichever `p1` the merge happens to list first.
describe("an account with more than one device", () => {
  let resetDeviceContexts;

  afterEach(() => {
    if (resetDeviceContexts) resetDeviceContexts();
    resetDeviceContexts = null;
    App.devices = [];
    App.selectedDeviceId = null;
  });

  it("seeds its strip from the home device's row", async () => {
    const contexts = await import("../src/core/deviceContexts.js");
    resetDeviceContexts = contexts.resetDeviceContexts;
    App.devices = [
      { id: "dev-2", name: "Desktop", status: "online" },
      { id: "dev-1", name: "This device", status: "online" },
    ];
    App.selectedDeviceId = "dev-1"; // home is the device the pick names
    contexts.adoptDeviceSession({
      deviceId: "dev-1",
      call: (...args) => bridge.call(...args),
      close: () => {},
      peer: () => {},
      onCarrier: () => {},
    });
    // The desktop's own build/login sorts first in the merge.
    const theirs = { ...branchRow({ agents: [agent({ id: "ag-9", ordinal: 9 })] }), deviceId: "dev-2" };
    const mine = { ...branchRow(), deviceId: "dev-1" };
    feedSnapshot = {
      items: [theirs, mine],
      projects: [],
      devices: { "dev-2": { items: [theirs], projects: [] }, "dev-1": { items: [mine], projects: [] } },
    };
    // The first read never answers, so what is painted is the seed alone.
    bridge.call = vi.fn(async (method) => {
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return new Promise(() => {});
      return {};
    });
    await mount();

    const strip = bubbles().filter((bubble) => bubble.dataset.bubble === "agent");
    expect(strip.map((bubble) => bubble.dataset.agent)).toEqual(["ag-1"]);
  });
});

// The two bodies a conversation points at rather than carries: the bytes of an
// attachment, and the contents of a diff revision. Neither is in the cache —
// they are immutable and they are big — so they are fetched when the reader
// asks and held in memory for the session.
describe("what the conversation points at", () => {
  const revisionMessage = {
    id: "m-1",
    type: "message",
    data: {
      sequence: 1,
      role: "user",
      body: "looks right",
      created_at: "2026-08-30T12:00:00Z",
      resolved_by_revision: "revision-2",
    },
  };

  it("reads a revision once, however often the reader opens it", async () => {
    payload = branchRow({ run: { run_id: "run-3", thread: { items: [revisionMessage], sessions: [] } } });
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "thread.revision") return { revision_id: "revision-2", contents: "+renamed" };
      return {};
    });
    await mount();

    const open = async () => {
      railHost().querySelector(".thread-revision-link").click();
      await flush();
    };
    await open();
    expect(railHost().querySelector(".thread-revision-view").textContent).toContain("+renamed");

    railHost().querySelector(".thread-revision-view button").click();
    await open();

    expect(railHost().querySelector(".thread-revision-view").textContent).toContain("+renamed");
    expect(callsTo("thread.revision")).toHaveLength(1);
  });
});

// The project's rail: the agent you talk to ABOUT a project, whose conversation
// stands in a scratch directory of its own. What a NEW one starts on is the
// DEVICE's project-agent setting (core/projectAgentSetting.js) — the project
// agent is the one agent that talks about a project instead of working in a
// checkout, and the harness for that job is often not the one coding work
// leads with. The setting is persistent, so no browser is asked at first use.
// On the project's own page the bubbles ARE the project's agent, so they wear
// what that agent wears above the line on a workspace's strip: the project's
// initial in a squared-off bubble, not a work item's pattern.
describe("the project's own page", () => {
  it("dresses the project's agent in the project's initial", async () => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      return method === "models.list" ? CATALOG : {};
    });
    payload = { entity_id: "run-project", project_id: "p1", agents: [agent({ id: "ag-project", ordinal: 1, state: "idle" })] };
    await writeRailBoard({ items: [payload], projects: [{ project_id: "p1", name: "build" }] });
    await mount({ kind: "project", projectId: "p1", entityId: "run-project" });
    await flush();

    const bubble = railHost().querySelector('[data-bubble="agent"]');
    expect(bubble.classList.contains("rail-bubble-project")).toBe(true);
    expect(bubble.querySelector(".rail-bubble-label").textContent).toBe("B");
    expect(bubble.querySelector("canvas")).toBeNull();
  });

  it("dresses the ghost the same way before the agent is born", async () => {
    bridge.call = vi.fn(async (method) => (method === "models.list" ? CATALOG : {}));
    payload = { entity_id: "run-project", project_id: "p1", agents: [] };
    await writeRailBoard({ items: [payload], projects: [{ project_id: "p1", name: "build" }] });
    await mount({ kind: "project", projectId: "p1", entityId: "run-project" });
    await flush();

    const ghost = railHost().querySelector('[data-bubble="ghost"]');
    expect(ghost.classList.contains("rail-bubble-project")).toBe(true);
    expect(ghost.querySelector(".rail-bubble-label").textContent).toBe("B");
  });
});

describe("a new agent on a project's rail", () => {
  const cards = () => [...railHost().querySelectorAll(".rail-newagent .rail-harness-choice")];
  const card = (provider) => cards().find((entry) => entry.dataset.provider === provider);
  const chosenCard = () => cards().find((entry) => entry.classList.contains("chosen"));

  const mountProjectRail = async (settings = {}) => {
    bridge.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "settings.get") return { default_harness: "claude_adk", ...settings };
      if (method === "agent.add") {
        return { entity_id: "run-project", agent: agent({ id: "ag-project", ordinal: 1, state: "idle" }) };
      }
      if (method === "thread.post") return { posted_sequence: 1 };
      if (method === "agent.start") return { agent_id: "ag-project" };
      return {};
    });
    // The project's conversation is a row on this machine like any other: the
    // page mints the owner, the sync layer writes the row, the rail reads it.
    payload = { entity_id: "run-project", project_id: "p1", agents: [] };
    await mount({ kind: "project", projectId: "p1", entityId: "run-project" });
  };

  const send = async (body) => {
    panel().querySelector("#railinput").value = body;
    panel().querySelector("#railsend").click();
    await flush();
  };

  it("starts on the harness, model and effort the device chose for project agents", async () => {
    // The browser's own defaults are about coding work and say nothing here.
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "claude_adk", harnesses: { claude: { model: "claude-opus-5", effort: "high" } },
    }));
    await mountProjectRail({ project_agent: { provider: "codex", model: "gpt-5.6-sol", effort: "medium" } });

    expect(chosenCard().dataset.provider).toBe("codex");
    await send("what is in this project?");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      entity_id: "run-project", provider: "codex", model: "gpt-5.6-sol", effort: "medium",
    });
  });

  it("falls back to the device's own default harness while the setting names none", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({
      provider: "codex", harnesses: { codex: { model: "gpt-5.6-sol", effort: "medium" } },
    }));
    await mountProjectRail();

    expect(chosenCard().dataset.provider).toBe("claude_adk");
    await send("what is in this project?");
    const { params } = callsTo("agent.add")[0];
    expect(params).toMatchObject({ entity_id: "run-project", provider: "claude_adk" });
    // Nothing else is named: the harness's own model and effort stand.
    expect(params.model).toBeUndefined();
    expect(params.effort).toBeUndefined();
  });

  // Pressing a card is a change of harness, and a model belongs to its harness:
  // the device's model comes along only on the harness the device chose.
  it("carries the device's model only onto the harness the device named", async () => {
    await mountProjectRail({ project_agent: { provider: "claude_adk", model: "claude-haiku-4-5" } });
    expect(chosenCard().dataset.provider).toBe("claude_adk");

    card("codex").click();
    await flush();
    expect(chosenCard().dataset.provider).toBe("codex");
    card("claude_adk").click();
    await flush();

    await send("what is in this project?");
    expect(callsTo("agent.add")[0].params).toMatchObject({
      provider: "claude_adk", model: "claude-haiku-4-5",
    });
  });
});
