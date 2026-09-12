// @vitest-environment jsdom
// The agent rail's wiring: the strip that is always there, the panel that
// expands beside it, the two faces of an agent, and the first message — which
// on a checkout Build owns nothing in is what brings the agent into being.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";

// The conversation cache writes through IndexedDB; give the module a fake one
// before anything imports it.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");

const refreshFeed = vi.fn(async () => {});
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
  primaryRunIdFor: () => null,
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
const { currentCacheScope, setCacheDevice } = await import("../src/core/cacheScope.js");
const { createChatRepository } = await import("../src/core/chatRepository.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { motionSettled } = await import("../src/core/motion.js");
const { insertRecord, resetOptimistic, runOptimistic } = await import("../src/core/optimistic.js");
const { SURFACE_PILL_GRACE_MS, writeOpenSurface } = await import("../src/core/agentSurfacesModel.js");
const { surfacesCacheAddress, surfacesRecord } = await import("../src/core/surfacesCache.js");
const { createAgentSelection } = await import("../src/core/agentSelection.js");
const { createAdoptingCall } = await import("../src/core/adoption.js");
const { FIRST_PAGE_ITEMS } = await import("../src/core/thread.js");
const { ACTIVITY_RECORD_KIND } = await import("../src/core/activityRuns.js");

const agent = (over = {}) => ({
  id: "ag-1", ordinal: 1, provider: "claude_adk", state: "live",
  unread_count: 0, unread_reason: null, working: false, ...over,
});

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
let calls = [];
let rail = null;

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((done) => setTimeout(done, 0));
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

const mount = async (context = { kind: "branch", projectId: "p1", branch: "build/login" }) => {
  rail = mountAgentRail(railHost(), context);
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  resetOptimistic();
  setCacheDevice("dev-1");
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
  App.modelCatalog = null; // fetched once per session; each test gets its own
  App.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (method === "models.list") return CATALOG;
    if (method === "branch.get") return payload;
    if (method === "issue.get") return payload;
    if (method === "run.adopt") return { run_id: "run-9" };
    if (method === "agent.start") return { agent_id: params.agent_id || "ag-new", term_id: `agent:${params.agent_id || "ag-new"}` };
    if (method === "agent.add") return { entity_id: "run-3", agent: agent({ id: "ag-2", ordinal: 2, state: "idle" }) };
    if (method === "thread.post") return { posted_sequence: 7 };
    return {};
  });
  App.chatRepository = createChatRepository({ scope: currentCacheScope(), call: (method, params) => App.call(method, params) });
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
  App.chatRepository?.dispose();
  App.chatRepository = null;
  vi.useRealTimers();
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
    expect(bubbles()[1].classList.contains("active")).toBe(true);
    bubbles()[1].click();
    await flush();
    expect(panel()).toBe(null);
    // …and the strip is still there with the panel shut.
    expect(bubbles().length).toBe(3);
  });

  // The rail reads the row every 1.6s and nearly every read says the same
  // thing. A rewrite then swaps the button a press is landing on for an
  // identical one, and the press is swallowed.
  it("leaves the bubbles alone on a tick that reads the same agents", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const before = bubbles();

    vi.advanceTimersByTime(1600);
    await flush();

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

    vi.advanceTimersByTime(1600);
    await flush();

    expect(bubbles()[0]).toBe(before);
    expect(countOn(bubbles()[0]).textContent).toBe("3");
    expect(countOn(bubbles()[0]).hidden).toBe(false);
    expect(bubbles()[0].classList.contains("working")).toBe(true);
    expect(bubbles()[0].title).toContain("3 unread");
    expect(bubbles()[0].getAttribute("aria-label")).toContain("3 unread");

    // …and back again: the count goes, the animation stops where it was.
    payload = branchRow({ agents: [agent()] });
    vi.advanceTimersByTime(1600);
    await flush();
    expect(bubbles()[0]).toBe(before);
    expect(countOn(bubbles()[0]).hidden).toBe(true);
    expect(bubbles()[0].classList.contains("working")).toBe(false);
  });

  it("keeps each bubble's element when another agent joins the strip", async () => {
    await mount();
    const before = bubbles()[0];
    const painter = livePainters()[0];
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });

    vi.advanceTimersByTime(1600);
    await flush();

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
    // accessible name — so two agents are still tellable apart in words.
    expect(first.getAttribute("aria-label")).toBe("Claude Code 1");
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
    expect(chooser.querySelector(".chooser-card.chosen").dataset.provider).toBe("codex");
    expect(railHost().querySelector(".rail-who").textContent).toBe("New agent");
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
    expect(railHost().querySelector(".rail-newagent .chooser-card.chosen").dataset.provider).toBe("claude_adk");
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
    // The rail opens on the first agent, and its next poll asks about it.
    expect(selection.get()).toBe("ag-1");
    vi.advanceTimersByTime(2000);
    await flush();
    expect(callsTo("branch.get").at(-1).params).toMatchObject({ agent_id: "ag-1" });

    bubbles()[1].click();
    await flush();
    expect(selection.get()).toBe("ag-2");
    vi.advanceTimersByTime(2000);
    await flush();
    expect(callsTo("branch.get").at(-1).params).toMatchObject({ agent_id: "ag-2" });
  });

  it("lets go of an agent the work item no longer has, instead of asking after it forever", async () => {
    const selection = createAgentSelection();
    await mount({ kind: "branch", projectId: "p1", branch: "build/login", selection });
    expect(selection.get()).toBe("ag-1");

    // The run behind the branch was replaced: the daemon refuses the id rather
    // than answering with somebody else's conversation.
    const refusing = App.call;
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "branch.get" && params.agent_id) throw new Error("unknown agent_id: ag-1");
      return refusing(method, params);
    });
    vi.advanceTimersByTime(2000);
    await flush();
    expect(selection.get()).toBe(null);

    // …and the next read asks the question that can be answered.
    vi.advanceTimersByTime(2000);
    await flush();
    expect(callsTo("branch.get").at(-1).params.agent_id).toBe(undefined);
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
    vi.advanceTimersByTime(1600);
    await flush();
    expect(livePainters()[0].working).toBe(true);

    payload = branchRow({ agents: [agent()] });
    vi.advanceTimersByTime(1600);
    await flush();
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
    vi.advanceTimersByTime(1600);
    await flush();
    expect(painter.dimmed).toBe(true);
    expect(painter.ink).toBeTruthy();
    expect(painter.ink).not.toBe(resting);
    expect(countOn(bubbles()[0]).hidden).toBe(false);
    expect(countOn(bubbles()[0]).textContent).toBe("2");

    payload = branchRow({ agents: [agent()] });
    vi.advanceTimersByTime(1600);
    await flush();
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

    vi.advanceTimersByTime(1600);
    await flush();

    expect(livePainters()).toEqual([painter]);
  });

  it("lets go of the painter for an agent that left the strip", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
    await mount();
    const [, second] = livePainters();

    payload = branchRow({ agents: [agent()] });
    vi.advanceTimersByTime(1600);
    await flush();

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
  const confirmModal = () => document.getElementById("confirm-scrim");

  const openSecondAgent = async () => {
    payload = twoAgents();
    await mount();
    bubbles()[1].click();
    await flush();
  };

  const holdRemove = () => {
    const answering = App.call;
    let refuse = null;
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "agent.remove") return new Promise((_, reject) => { refuse = reject; });
      return answering(method, params);
    });
    return { refuse: (error) => refuse(error) };
  };
  const railBodyNow = () => railHost().querySelector("#rail-body");
  const confirmEveryModal = () => {
    document.querySelectorAll(".modal-scrim").forEach((scrim) => {
      const ok = scrim.querySelector("[data-confirm-ok]");
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
    vi.advanceTimersByTime(1600);
    await flush();

    expect(callsTo("agent.remove")[0].params).toEqual({ entity_id: "run-3", agent_id: "ag-1" });
    // A working branch, not a broken one: the strip drops to its ghost and the
    // panel head offers to start a new agent.
    expect(bubbles().map((b) => b.dataset.bubble)).toEqual(["ghost"]);
    expect(panel().querySelector(".rail-who").textContent).toBe("New agent");
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("offers no removal on an issue's one agent", async () => {
    payload = { issue_id: "plan-1", project_id: "p1", agents: [agent()], thread: { items: [] } };
    await mount({ kind: "issue", projectId: "p1", issueId: "plan-1" });
    expect(removeButton()).toBe(null);
  });

  it("asks before it removes, and does nothing at all when the answer is no", async () => {
    await openSecondAgent();
    removeButton().click();
    await flush();
    expect(confirmModal()).toBeTruthy();
    confirmModal().querySelector("[data-confirm-cancel]").click();
    await flush();
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 1");
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
    expect(removeButton()).toBeTruthy();
    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
  });

  it("takes the agent off the moment the answer is yes, before the daemon replies", async () => {
    payload = twoAgents();
    await mount();
    payload = twoAgents({
      run: {
        run_id: "run-3",
        thread: { items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "agent", body: "words from the second agent" } }], sessions: [] },
      },
    });
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 1");
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
    expect(panel().querySelector(".rail-who").textContent).toBe("New agent");
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("does not resurrect the agent when a read lands mid-flight still listing it", async () => {
    await openSecondAgent();
    holdRemove();
    removeButton().click();
    await flush();
    confirmModal().querySelector("[data-confirm-ok]").click();
    await flush();
    const readsBefore = callsTo("branch.get").length;

    vi.advanceTimersByTime(1600);
    await flush();

    expect(callsTo("branch.get").length).toBeGreaterThan(readsBefore);
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "branch.get") throw new Error("the bridge is not answering");
      return answering(method, params);
    });

    held.refuse(new Error("agent is mid-spawn"));
    await flush();

    expect(bubbles().map((b) => b.dataset.agent)).toEqual(["ag-1", "ag-2", ""]);
    expect(bubbles()[1].classList.contains("active")).toBe(true);
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
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
  it("carries the agent, the one way down to its screen, and a box to write in", async () => {
    await mount();
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 1");
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
    vi.advanceTimersByTime(1600);
    await flush();

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
    vi.advanceTimersByTime(1600);
    await flush();

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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    vi.advanceTimersByTime(1600);
    await flush();

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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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

    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
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
    vi.advanceTimersByTime(1600);
    await flush();

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
    const twoAgents = () => {
      const said = (who) => ({
        items: [{ type: "message", data: { role: "agent", body: `words from ${who}`, seen_at: null } }],
        sessions: [],
      });
      App.call = vi.fn(async (method, params) => {
        calls.push({ method, params });
        if (method === "branch.get") {
          return branchRow({
            agents: [agent(), agent({ id: "ag-2", ordinal: 2 })],
            run: { run_id: "run-3", thread: said(params.agent_id || "ag-1") },
          });
        }
        return {};
      });
    };

    it("reads the newly opened agent's conversation at once, not on the next poll", async () => {
      twoAgents();
      await mount();
      const before = callsTo("branch.get").length;

      bubbles()[1].click();
      await flush();

      const reads = callsTo("branch.get");
      expect(reads.length).toBeGreaterThan(before);
      expect(reads.at(-1).params.agent_id).toBe("ag-2");
    });

    it("shows the conversation of the agent it switched to", async () => {
      twoAgents();
      await mount();
      expect(panel().textContent).toContain("words from ag-1");

      bubbles()[1].click();
      await flush();

      expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
      expect(panel().textContent).toContain("words from ag-2");
      expect(panel().textContent).not.toContain("words from ag-1");
    });

    it("never shows one agent's words under another's name while the read is in flight", async () => {
      twoAgents();
      await mount();
      expect(panel().textContent).toContain("words from ag-1");

      // The press repaints before its read can answer. Whatever the panel draws
      // in that gap, it must not be the conversation of the agent just left.
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

  const pagedConversation = (hasMore, moreAboveThatPage = true, agents = [agent()]) => {
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "branch.get") {
        // A cursored poll is a forward delta and says nothing about the far
        // end of the conversation — only a paged answer does.
        const thread = params.thread_after_sequence
          ? { sessions: [], items: [], thread_total: 99, thread_last_sequence: 99 }
          : firstPage(hasMore);
        return branchRow({ agents, run: { run_id: "run-3", thread } });
      }
      if (method === "thread.page") return pageAbove(moreAboveThatPage);
      return {};
    });
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
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") {
        return branchRow({
          run: {
            run_id: "run-3",
            thread: params.thread_after_sequence
              ? { sessions: [], items: [], thread_total: 1530, thread_last_sequence: 1530 }
              : thread,
          },
        });
      }
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
  it("pins nothing when the feed row has nothing to report", async () => {
    await mount();
    expect(railStatus().hidden).toBe(true);
    expect(railStatus().textContent.trim()).toBe("");
  });

  it("pulses and clocks the turn while the branch is working", async () => {
    payload = branchRow({ agents: [agent({ working_time: { since: new Date(Date.now() - 750000).toISOString(), seconds: 750 } })] });
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login",
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
        kind: "branch", project_id: "p1", branch: "build/login",
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
        kind: "branch", project_id: "p1", branch: "build/login",
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
        kind: "branch", project_id: "p1", branch: "build/login",
        working: true, working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 }, stat: null,
      }],
      projects: [],
    });
    payload = branchRow({
      ...payload,
      agents: [agent({ ...payload.agents[0], working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 } })],
    });
    vi.advanceTimersByTime(1600);
    await flush();
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
        kind: "branch", project_id: "p1", branch: "build/login",
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
    const shared = createAdoptingCall((method, params) => App.call(method, params), "p1", "wt-3");
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
  const cards = () => [...railHost().querySelectorAll(".rail-newagent .chooser-card")];
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
    vi.advanceTimersByTime(1600);
    await flush();
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
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
    App.modelCatalog = { ...CATALOG, default_provider: "claude" };
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
    App.modelCatalog = { ...CATALOG, default_provider: "claude" };
    payload = agentless();
    await mount();
    card("claude").click();
    await flush();

    rail.dispose();
    App.modelCatalog = CATALOG;
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

    vi.advanceTimersByTime(1600);
    await flush();

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
    vi.advanceTimersByTime(1600);
    await flush();

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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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

  it("asks for a model when an agent has never run and chose nothing", async () => {
    payload = branchRow({ agents: [agent({ model: "", effort: "", active_model: "" })] });
    await mount();

    expect(modelMenuButton().textContent).toContain("Select model");
  });

  it("moves the label the instant a model is picked, before agent.choose answers", async () => {
    payload = branchRow({ agents: [agent({ model: "claude-opus-5" })] });
    await mount();
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    let acknowledgeChoice;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answering = App.call;
    App.call = vi.fn(async (method, params) => {
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
    vi.advanceTimersByTime(1600);
    await flush();

    expect(send().dataset.action).toBe("send");
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("half a sent");

    // …and back to the plain button when the turn it could have stopped ends.
    payload = branchRow({ agents: [agent({ working: false, can_interrupt: true })] });
    vi.advanceTimersByTime(1600);
    await flush();
    expect(send().dataset.action).toBe("send");
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("half a sent");
  });
});

describe("the conversation's local cache", () => {
  const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3" }];
  const threadItem = (sequence, body) => ({
    id: `m-${sequence}`,
    type: "message",
    data: { sequence, role: "user", body, created_at: "2026-08-30T12:00:00Z" },
  });

  it("seeds the saved window, so opening the chat asks for a delta, history in hand", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" },
      { items: [threadItem(1, "what was said before")], olderItemsRemain: false, deliveredSequence: 1, knownTotalItems: 1 },
    );
    feedSnapshot = { items: feedItems, projects: [] };
    await mount();
    await flush(); // the auto-selected agent's seed lands before the next poll
    vi.advanceTimersByTime(1600);
    await flush();
    const delta = callsTo("branch.get").find((call) => call.params.agent_id === "ag-1");
    expect(delta.params.thread_after_sequence).toBe(1);
    expect(delta.params.thread_limit).toBeUndefined();
  });

  // The count on a folded run comes off the window, so the window on disk
  // carries it: a reader coming back to a long run sees the bridge's number on
  // the seeded paint, not a count of the handful of rows the disk held.
  it("seeds the digests with the window, so the fold's count survives the visit", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" },
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

  it("writes the conversation's window through for the next visit", async () => {
    feedSnapshot = { items: feedItems, projects: [] };
    payload = branchRow({
      run: {
        run_id: "run-3",
        thread: { items: [threadItem(2, "fresh words")], has_more: false, thread_total: 1, thread_last_sequence: 2, sessions: [] },
      },
    });
    await mount();
    bubbles()[0].click();
    await flush();
    const record = await readCached({ deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" });
    expect(record.value.items).toHaveLength(1);
    expect(record.value.items[0].data.body).toBe("fresh words");
    expect(record.value.deliveredSequence).toBe(2);
  });
});

describe("revisiting a conversation", () => {
  const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3" }];
  const historyThread = () => ({
    items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "user", body: "the history", created_at: "2026-08-30T12:00:00Z" } }],
    has_more: false,
    thread_total: 1,
    thread_last_sequence: 1,
    sessions: [],
  });

  it("stands the strip and the saved conversation up before the first read answers", async () => {
    await writeCached(
      { deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" },
      { items: historyThread().items, olderItemsRemain: false, deliveredSequence: 1, knownTotalItems: 1 },
    );
    feedSnapshot = { items: [{ ...feedItems[0], agents: [agent()] }], projects: [] };
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return new Promise(() => {});
      return {};
    });
    await mount();
    await flush();
    expect(bubbles().length).toBeGreaterThan(0);
    expect(railHost().querySelector("#rail-body").textContent).toContain("the history");
  });

  // The deployed bug: the rail remembers which agent was open across remounts,
  // but the cache's owner marker started blank — so the first threadFor of a
  // revisit wiped the window the seed had just opened, after its delta cursor
  // was already sent. The empty delta painted "No conversation yet" until the
  // 60s safety poll.
  it("paints the saved history at once, and an empty delta does not blank it", async () => {
    feedSnapshot = { items: feedItems, projects: [] };
    payload = branchRow({ run: { run_id: "run-3", thread: historyThread() } });
    await mount();
    await flush(); // the first visit auto-selects ag-1 and persists its window
    rail.dispose();
    rail = null;

    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") {
        // The revisit's delta cursor: nothing new since the saved window.
        if (params.thread_after_sequence != null)
          return branchRow({ run: { run_id: "run-3", thread: { items: [], thread_total: 1, thread_last_sequence: 1, sessions: [] } } });
        return branchRow({ run: { run_id: "run-3", thread: historyThread() } });
      }
      return {};
    });
    await mount();
    await flush();
    const delta = callsTo("branch.get").find((call) => call.params.thread_after_sequence != null);
    expect(delta).toBeTruthy();
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
  const feedItems = [{
    kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-3", worktree_id: "wt-3",
    agents: [agent(), agent({ id: "ag-2", ordinal: 2 })],
  }];
  const railContext = { kind: "branch", projectId: "p1", branch: "build/login" };
  const shellsRunning = (...descriptions) => ({
    shells: descriptions.map((description, index) => ({ id: `sh-${index}`, description, state: "running", tail: [] })),
  });
  const aChecklist = { checklist: [{ id: "t-1", subject: "wire the seed", state: "in_progress" }] };
  const surfacesAddress = (sub) => surfacesCacheAddress({ deviceId: "dev-1", entityId: "run-3", agentId: sub });
  const saveSurfaces = (sub, surfaces) => writeCached(surfacesAddress(sub), surfacesRecord(surfaces));
  const saveSurfacesLongAgo = async (sub, surfaces) => {
    const clock = vi.spyOn(Date, "now").mockReturnValue(Date.now() - SURFACE_PILL_GRACE_MS - 1);
    await saveSurfaces(sub, surfaces);
    clock.mockRestore();
  };
  const savedSurfaces = (sub) => readCached(surfacesAddress(sub));
  const savedDescription = async (sub) => (await savedSurfaces(sub)).value.surfaces.shells[0].description;
  const menuKinds = () =>
    [...railHost().querySelectorAll(".rail-surface-menu .mi")].map((item) => item.dataset.action);
  const pillKinds = () =>
    [...railStatusPills().querySelectorAll(".surface-pill")].map((pill) => pill.dataset.surfaceKind);
  const pillCount = (kind) =>
    railStatusPills().querySelector(`[data-surface-kind="${kind}"] .surface-pill-count`).textContent.trim();
  const answerNothing = () => {
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return new Promise(() => {});
      return {};
    });
  };

  beforeEach(() => {
    feedSnapshot = { items: feedItems, projects: [] };
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
  });

  it("seeds the same snapshot whole while the grace still holds", async () => {
    await saveSurfaces("ag-1", { ...shellsRunning("cargo test"), ...aChecklist });
    answerNothing();
    await mount();
    expect(pillKinds()).toEqual(["shells", "checklist"]);
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
        { deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" },
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
  });

  it("drops a seed whose agent was left while the read was in flight", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    rail = mountAgentRail(railHost(), railContext);
    bubbles()[1].click(); // ag-1's seed is still in flight
    await flush();
    expect(pillKinds()).toEqual([]);
  });

  it("replaces the seeded pills with the first live payload", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test", "cargo clippy"));
    let answer = null;
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "branch.get") return new Promise((resolve) => { answer = resolve; });
      return {};
    });
    await mount();
    expect(pillCount("shells")).toBe("2");

    answer(branchRow({ agents: [agent({ surfaces: shellsRunning("cargo test") })] }));
    await flush();
    expect(pillCount("shells")).toBe("1");
  });

  it("writes the snapshot a payload moved, and rewrites nothing while it holds still", async () => {
    payload = branchRow({ agents: [agent({ surfaces: shellsRunning("cargo test") })] });
    await mount();
    expect(await savedDescription("ag-1")).toBe("cargo test");

    await saveSurfaces("ag-1", shellsRunning("left by another hand"));
    vi.advanceTimersByTime(1600);
    await flush();
    expect(await savedDescription("ag-1")).toBe("left by another hand");

    payload = branchRow({ agents: [agent({ surfaces: shellsRunning("cargo clippy") })] });
    vi.advanceTimersByTime(1600);
    await flush();
    expect(await savedDescription("ag-1")).toBe("cargo clippy");
  });

  it("leaves the record alone for a payload carrying no surfaces at all", async () => {
    await saveSurfaces("ag-1", shellsRunning("from the last visit"));
    payload = branchRow({ agents: [agent()] });
    await mount();
    expect(await savedDescription("ag-1")).toBe("from the last visit");
  });

  it("addresses the conversation and its surfaces alike, the kind apart", async () => {
    payload = branchRow({
      agents: [agent({ surfaces: shellsRunning("cargo test") })],
      run: {
        run_id: "run-3",
        thread: {
          items: [{ id: "m-1", type: "message", data: { sequence: 1, role: "user", body: "hello", created_at: "2026-08-30T12:00:00Z" } }],
          has_more: false, thread_total: 1, thread_last_sequence: 1, sessions: [],
        },
      },
    });
    await mount();
    expect(await readCached({ deviceId: "dev-1", entityId: "ag-1", kind: "thread", sub: "" })).toBeTruthy();
    expect(await savedSurfaces("ag-1")).toBeTruthy();
  });

  it("leaves no seed landing after the rail is gone", async () => {
    await saveSurfaces("ag-1", shellsRunning("cargo test"));
    answerNothing();
    rail = mountAgentRail(railHost(), railContext);
    rail.dispose();
    rail = null;
    await flush();
    expect(railHost().querySelector(".surface-pill")).toBe(null);
    expect(notifyError).not.toHaveBeenCalled();
  });
});

describe("a workspace conversation on a metadata-only bridge", () => {
  it("recovers the exact adopted run and posts through its agent conversation", async () => {
    const run = {
      run_id: "run-3",
      project_id: "p1",
      branch: "build/login",
      agents: [agent({ id: "ag-workspace", conversation_id: "conversation-workspace", state: "live" })],
      thread: { items: [], sessions: [] },
    };
    App.call = vi.fn(async (method, params) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return { id: "run-3", project_id: "p1", directories: [{ branch: "build/login" }] };
      if (method === "run.get") return run;
      if (method === "thread.post") return {
        entity_id: "run-3", agent_id: "ag-workspace", conversation_id: "conversation-workspace", posted_sequence: 7,
      };
      return {};
    });

    await mount({ kind: "workspace", projectId: "p1", workspaceId: "run-3", sourceId: "root" });
    const input = railHost().querySelector("#railinput");
    expect(input).not.toBeNull();
    input.value = "fix the deployed workspace";
    railHost().querySelector("#railsend").click();
    await flush();

    expect(callsTo("run.get")[0].params).toMatchObject({ run_id: "run-3" });
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3",
      agent_id: "ag-workspace",
      conversation_id: "conversation-workspace",
      body: "fix the deployed workspace",
    });
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
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
      if (method !== "agent.add") return answer(method, params);
      calls.push({ method, params });
      await held;
      return { entity_id: "run-3", agent: agent({ id: "ag-2", ordinal: 2, state: "idle" }) };
    });
    return release;
  };

  const refuseCall = (refused, message) => {
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const running = App.chatRepository.optimisticStore().runOptimistic({
      scope: `${App.chatRepository.scopeKey}:agents:branch:p1:build/login`,
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
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 1");
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
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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

    vi.advanceTimersByTime(1600);
    await flush();

    expect(bubbles()[0]).toBe(bubble);
    expect(painter.destroyed).toBe(false);
    expect(document.activeElement).toBe(bubble);
  });

  it("leaves the optimistic agent standing when a read lands mid-flight", async () => {
    payload = agentless();
    await mount();
    const release = holdAgentAdd();
    await press("start here");
    const bubble = bubbles()[0];
    const reads = callsTo("branch.get").length;

    vi.advanceTimersByTime(3200);
    await flush();

    expect(bubbles()[0]).toBe(bubble);
    expect(bubbles()).toHaveLength(2);
    expect(callsTo("branch.get").length).toBeGreaterThan(reads);
    expect(callsTo("branch.get").every((call) => call.params.agent_id === undefined)).toBe(true);

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
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
      if (method !== "thread.post") return answer(method, params);
      calls.push({ method, params });
      await held;
      return { posted_sequence: 7 };
    });
    return release;
  };

  const refuseThreadPost = (message) => {
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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

  it("shows the message and clears the box before thread.post answers", async () => {
    payload = branchRow({ agents: [agent({ state: "live" })] });
    await mount();
    const release = holdThreadPost();

    await press("look at the login flow");

    expect(timeline().textContent).toContain("look at the login flow");
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

    vi.advanceTimersByTime(1600);
    await flush();

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
    vi.advanceTimersByTime(1600);
    await flush();

    expect(copiesOf("look at the login flow")).toBe(1);
  });

  it("leaves a delivered message where it landed when only the wake is refused", async () => {
    payload = branchRow({ agents: [agent({ state: "exited" })] });
    await mount();
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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
    const answer = App.call;
    App.call = vi.fn(async (method, params) => {
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
        kind: "branch", project_id: "p1", branch: "build/login",
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
    expect(phoneRule[1]).toContain("right:var(--agent-strip)");
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

  it("shimmers the status clock and every ticking row clock through one rule, holding still under reduced motion", () => {
    expect(shellCss.match(/@keyframes clock-shimmer/g)).toHaveLength(1);
    expect(shellCss.match(/linear-gradient\(100deg/g)).toHaveLength(1);
    const shimmerRule = shellCss.match(
      /\n\.rail-status-working, \.surface-row-clock\[data-running-since\] \{ color:transparent;[^}]*\}/,
    );
    expect(shimmerRule[0]).toContain("animation:clock-shimmer");
    expect(shimmerRule[0]).toContain("background-clip:text");
    expect(shimmerRule[0]).toContain("var(--clock-ink)");
    const stillRule = shellCss.match(
      /@media \(prefers-reduced-motion: reduce\) \{\n?\s*\.rail-status-working, \.surface-row-clock\[data-running-since\] \{[^}]*\}/,
    );
    expect(stillRule).not.toBe(null);
    expect(stillRule[0]).toContain("animation:none");
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
    vi.advanceTimersByTime(2000);
    await flush();
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
      vi.advanceTimersByTime(1600);
      await flush();
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
    expect(composerRule).toMatch(/position:relative/);
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
    App.call.mockImplementation(async (method, params) => {
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

    payload = branchRow({
      run: {
        run_id: "run-3",
        thread: { sessions: [], items: [toolCall(52, "Read q.js")], thread_total: 4, thread_last_sequence: 52 },
      },
    });
    vi.advanceTimersByTime(1600);
    await flush();

    runHead().click();
    await flush();

    expect(callsTo("thread.activity")).toEqual([]);
    expect(
      await readCached({ deviceId: "dev-1", entityId: "run-3", kind: ACTIVITY_RECORD_KIND, sub: "ag-1:10" }),
    ).toBeUndefined();
    expect(runRows().map((row) => row.dataset.sequence)).toEqual(["50", "51", "52"]);
  });

  // A window that breaks — an item deleted, a delta dropped, the daemon
  // restarted — is let go for a refetch, and the very tick that let it go still
  // paints the rows and the digests it was drawn from. So the side that decides
  // what to fetch has to read the digests the paint read: reading the window
  // instead leaves the run the reader presses in that frame asking for nothing.
  it("asks over the digests the timeline was painted from, not the window's", async () => {
    payload = conversation(
      [said(1, "Have a look."), toolCall(50, "Read y.js"), toolCall(51, "Read z.js"), said(60, "Done.")],
      [{ from_sequence: 10, through_sequence: 51, tool_calls: 40, rows: 40, last_tool_call: null }],
    );
    answering({ items: [toolCall(10, "Read a.js")], oldest_sequence: 10, has_more: false });
    await mount();

    // One item shorter than the cache was told the conversation is: a deletion,
    // which is the one change no arrival ever unsays.
    payload = branchRow({
      run: { run_id: "run-3", thread: { sessions: [], items: [], thread_total: 3, thread_last_sequence: 60 } },
    });
    vi.advanceTimersByTime(1600);
    await flush();

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
    App.call.mockImplementation(async (method, params) => {
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

    vi.advanceTimersByTime(1600);
    await flush();

    expect(markSurvived()).toBe(true);
  });

  it("paints again the moment one of them does", async () => {
    payload = conversationOf([said(1, "the first thing said")]);
    await mount();
    markTheFirstRow();

    payload = conversationOf([said(1, "the first thing said"), said(2, "and the next")]);
    vi.advanceTimersByTime(1600);
    await flush();

    expect(markSurvived()).toBe(false);
    expect(railHost().querySelector(".thread-items").textContent).toContain("and the next");
  });
});
