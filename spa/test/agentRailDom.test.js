// @vitest-environment jsdom
// The agent rail's wiring: the strip that is always there, the panel that
// expands beside it, the two faces of an agent, and the first message — which
// on a checkout Build owns nothing in is what brings the agent into being.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// The conversation cache writes through IndexedDB; give the module a fake one
// before anything imports it.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

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
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args), notify: () => {} }));
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
const { setCacheDevice } = await import("../src/core/cacheScope.js");
const { readCached, writeCached, wipeCache } = await import("../src/core/localCache.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { createAgentSelection } = await import("../src/core/agentSelection.js");
const { createAdoptingCall } = await import("../src/core/adoption.js");
const { FIRST_PAGE_ITEMS } = await import("../src/core/thread.js");

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
    { id: "codex", label: "Codex", models: [], efforts: [] },
  ],
};

const railHost = () => document.getElementById("agent-rail");
const modelMenuButton = () => railHost().querySelector(".composer-model .caret");
const menuItem = (action) => railHost().querySelector(`.composer-model .mi[data-action="${action}"]`);
const bubbles = () => [...railHost().querySelectorAll(".rail-bubble")];
const livePainters = () => painters.filter((painter) => !painter.destroyed);
const countOn = (bubble) => bubble.querySelector(".rail-count");
const panel = () => railHost().querySelector(".rail-panel");
const callsTo = (method) => calls.filter((call) => call.method === method);
const railStatus = () => railHost().querySelector("#rail-status");

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
    if (method === "agent.start") return { agent_id: "ag-new", term_id: "agent:ag-new" };
    if (method === "agent.add") return { entity_id: "run-3", agent: agent({ id: "ag-2", ordinal: 2, state: "idle" }) };
    return {};
  });
});

afterEach(() => {
  if (rail) rail.dispose();
  rail = null;
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

  it("rebuilds the strip when the agents themselves change", async () => {
    await mount();
    const before = bubbles()[0];
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });

    vi.advanceTimersByTime(1600);
    await flush();

    expect(bubbles()).toHaveLength(3);
    expect(bubbles()[0]).not.toBe(before);
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
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "codex" });
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
  const twoAgents = () => branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2 })] });
  const removeButton = () => panel().querySelector(".rail-remove");
  const confirmModal = () => document.getElementById("confirm-scrim");

  const openSecondAgent = async () => {
    payload = twoAgents();
    await mount();
    bubbles()[1].click();
    await flush();
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
});

describe("the conversation panel", () => {
  it("carries the agent, both faces of it, and a box to write in", async () => {
    await mount();
    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 1");
    expect([...panel().querySelectorAll(".rail-mode")].map((m) => m.dataset.mode)).toEqual(["chat", "tui"]);
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  it("swaps the same panel onto the agent's screen, addressed by that agent", async () => {
    await mount();
    panel().querySelector('[data-mode="tui"]').click();
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
    panel().querySelector('[data-mode="tui"]').click();
    await flush();

    await mountAgentTab.mock.calls[0][2].onStart();

    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-3", agent_id: "ag-1" });
  });

  // The terminal is a capability, not a guarantee. A harness that reports its
  // own reasoning and tool calls is not opaque, so it has no basement to drop
  // into — and the rail is where that shows: no TUI button, and no way to ask
  // for one.
  it("offers the terminal only to an agent whose session has one", async () => {
    payload = branchRow({ agents: [agent({ has_terminal: false })] });
    await mount();
    expect([...panel().querySelectorAll(".rail-mode")].map((m) => m.dataset.mode)).toEqual(["chat"]);
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  // An older bridge does not mention the field at all, and silence is not a
  // refusal: every agent had a terminal before this question could be asked.
  it("keeps the terminal for a digest that never mentions one", async () => {
    await mount();
    expect(agent().has_terminal).toBe(undefined);
    expect([...panel().querySelectorAll(".rail-mode")].map((m) => m.dataset.mode)).toEqual(["chat", "tui"]);
  });

  // The face the panel wears is remembered per work item, so opening a
  // terminal-less agent's bubble arrives with "tui" in hand. It must land on
  // the conversation anyway, and attach nothing.
  it("puts the panel back on the conversation when a terminal-less agent is opened", async () => {
    payload = branchRow({ agents: [agent(), agent({ id: "ag-2", ordinal: 2, has_terminal: false })] });
    await mount();
    panel().querySelector('[data-mode="tui"]').click();
    await flush();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);

    bubbles()[1].click();
    await flush();

    expect(panel().querySelector(".rail-who").textContent).toBe("Claude Code 2");
    expect([...panel().querySelectorAll(".rail-mode")].map((m) => m.dataset.mode)).toEqual(["chat"]);
    expect(panel().querySelector("#railinput")).toBeTruthy();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);

    // …and the choice is not spent: the agent that does have a terminal is
    // still where it was left.
    bubbles()[0].click();
    await flush();
    expect(panel().querySelector('[data-mode="tui"]')).toBeTruthy();
    expect(mountAgentTab).toHaveBeenCalledTimes(2);
  });

  // The digest can change its answer under a panel that is already open — an
  // agent is replaced by one of another shape on the same bubble. The screen
  // has to go with it.
  it("takes the terminal away from a panel standing on one when the agent loses it", async () => {
    await mount();
    panel().querySelector('[data-mode="tui"]').click();
    await flush();
    expect(panel().querySelector("#railinput")).toBe(null);

    payload = branchRow({ agents: [agent({ has_terminal: false })] });
    vi.advanceTimersByTime(1600);
    await flush();

    expect(panel().querySelector('[data-mode="tui"]')).toBe(null);
    expect(panel().querySelector("#railinput")).toBeTruthy();
  });

  it("leaves a live screen alone while the rail keeps polling", async () => {
    await mount();
    panel().querySelector('[data-mode="tui"]').click();
    await flush();
    expect(mountAgentTab).toHaveBeenCalledTimes(1);
    const before = panel();
    vi.advanceTimersByTime(5000);
    await flush();
    // The same panel element, the same pane: a poll must not re-attach a PTY.
    expect(panel()).toBe(before);
    expect(mountAgentTab).toHaveBeenCalledTimes(1);
  });

  it("tells the daemon an agent's conversation has been read while it is open at the end", async () => {
    payload = branchRow({ agents: [agent({ unread_count: 2, unread_reason: "done" })] });
    await mount();
    // No floor: this conversation arrived whole, so the end of it is the end.
    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", null);
  });

  it("says nothing about reading a conversation with nothing waiting", async () => {
    await mount();
    expect(markSeen).not.toHaveBeenCalled();
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

    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 98);
  });

  it("moves the floor it reports down as the reader scrolls back", async () => {
    pagedConversation(true, true, [agent({ unread_count: 1, unread_reason: "agent_message" })]);
    await mount();
    markSeen.mockClear();

    railBody().dispatchEvent(new Event("scroll"));
    await flush();

    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1", 96);
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
    expect(railStatus().textContent).toBe("");
  });

  it("pulses and clocks the turn while the branch is working", async () => {
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login",
        working: true, working_time: { since: new Date(Date.now() - 750000).toISOString(), seconds: 750 }, stat: null,
      }],
      projects: [],
    });
    await mount();
    expect(railStatus().hidden).toBe(false);
    expect(railStatus().querySelector(".sdot-working")).toBeTruthy();
    expect(railStatus().textContent).toContain("Working 12m 30s");
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
    expect(railStatus().querySelector(".sdot-working")).toBeNull();
    expect(railStatus().textContent).toContain("+4 −1");
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
    expect(git.querySelector(".rail-status-sync").textContent).toBe("↓1");
    expect(git.querySelector(".rail-status-stat").textContent).toBe("+104 −38");
    expect(git.previousElementSibling.className).toBe("rail-status-working");
  });

  it("ticks the elapsed time between feed reads", async () => {
    // The ticker's Date.now() has to move with the fake clock for this one, so
    // this test fakes Date too — the others read `since` off the real clock at
    // mount and never advance timers far enough to notice the difference.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    await pushFeed({
      items: [{
        kind: "branch", project_id: "p1", branch: "build/login",
        working: true, working_time: { since: new Date(Date.now() - 5000).toISOString(), seconds: 5 }, stat: null,
      }],
      projects: [],
    });
    await mount();
    expect(railStatus().textContent).toContain("Working 5s");
    vi.advanceTimersByTime(3000);
    await flush();
    expect(railStatus().textContent).toContain("Working 8s");
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

  // Two agents, never three. Whether Claude Code opens as the TUI is the
  // account's question, answered once in Settings — putting it in front of
  // every human creating an agent is what this view stopped doing.
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
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "claude_adk" });
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
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "codex" });
  });

  it("adopts a checkout Build owns nothing in before it creates the agent", async () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
    await mount();

    await send("start here");

    expect(callsTo("run.adopt")[0].params).toMatchObject({ project_id: "p1", worktree_id: "wt-3" });
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-9", provider: "claude_adk" });
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
    expect(callsTo("agent.add")[0].params).toEqual({
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
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "claude" });
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
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "claude_adk" });
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

    expect(modelMenuButton().textContent).toContain("Claude Opus 5 · low");
    modelMenuButton().click();
    expect([...railHost().querySelectorAll(".composer-model .mi")].map((mi) => mi.dataset.action)).toEqual([
      "model:", "model:claude-opus-5", "model:claude-haiku-4-5", "effort:", "effort:low", "effort:high",
    ]);
  });

  it("persists the choice on the entity, saying nothing about the harness", async () => {
    payload = branchRow({ agents: [agent()] });
    await mount();

    modelMenuButton().click();
    menuItem("model:claude-opus-5").click();
    await flush();

    expect(callsTo("agent.choose")[0].params).toEqual({
      entity_id: "run-3", model: "claude-opus-5", effort: "",
    });
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

// A message to an agent mid-turn can be handed over two ways: queued for its
// next step — which for a carrier that can be steered usually decides the same
// turn — or after stopping the turn outright. Two behaviours behind one verb,
// so the send is a split button, and the stop is never the default press.
describe("interrupting the turn", () => {
  const splitSend = () => panel().querySelector(".composer-send-control .splitbtn");
  const menuItem = (action) =>
    [...panel().querySelectorAll(".composer-send-control .splitmenu .mi")].find((mi) => mi.dataset.action === action);

  it("offers the plain send to an agent whose turn cannot be stopped", async () => {
    payload = branchRow({ agents: [agent({ working: true })] });
    await mount();
    expect(splitSend()).toBe(null);
    expect(panel().querySelector("#railsend")).toBeTruthy();
  });

  it("offers the plain send to an agent that is not working, whatever it can do", async () => {
    payload = branchRow({ agents: [agent({ working: false, can_interrupt: true })] });
    await mount();
    expect(splitSend()).toBe(null);
  });

  it("splits the send for a working agent that announced the interrupt", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    expect(splitSend()).toBeTruthy();
    // The default press is the send it always was, still the button by that id.
    expect(splitSend().querySelector("#railsend").dataset.action).toBe("send");
    expect(menuItem("interrupt_send").textContent).toContain("Interrupt & send");
  });

  it("posts the message with the interrupt flag when that is the option chosen", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    panel().querySelector("#railinput").value = "stop, do this instead";
    splitSend().querySelector(".caret").click();
    menuItem("interrupt_send").click();
    await flush();
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: "run-3", agent_id: "ag-1", body: "stop, do this instead", interrupt: true,
    });
  });

  it("leaves the flag off the default press", async () => {
    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    await mount();
    panel().querySelector("#railinput").value = "when you get a moment";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("thread.post")[0].params.body).toBe("when you get a moment");
    expect(callsTo("thread.post")[0].params.interrupt).toBeUndefined();
  });

  // The condition changes every time an agent starts or finishes a turn, which
  // on a 1.6s poll is often. Rebuilding the composer to swap the control would
  // take the draft and the focus with it, mid-sentence.
  it("swaps the control in place, keeping the box and the words in it", async () => {
    payload = branchRow({ agents: [agent({ working: true })] });
    await mount();
    const input = panel().querySelector("#railinput");
    input.value = "half a sent";
    expect(splitSend()).toBe(null);

    payload = branchRow({ agents: [agent({ working: true, can_interrupt: true })] });
    vi.advanceTimersByTime(1600);
    await flush();

    expect(splitSend()).toBeTruthy();
    expect(panel().querySelector("#railinput")).toBe(input);
    expect(input.value).toBe("half a sent");

    // …and back to the plain button when the turn it could have stopped ends.
    payload = branchRow({ agents: [agent({ working: false, can_interrupt: true })] });
    vi.advanceTimersByTime(1600);
    await flush();
    expect(splitSend()).toBe(null);
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
      { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
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
    const record = await readCached({ deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" });
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
      { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "ag-1" },
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

// The agent's surfaces ride the digest the rail already reads, so the pills go
// where the reader is already looking: inside the pinned composer block,
// between the status line and the box. A row's action is an ordinary message
// out of the rail's one send path.
describe("the agent's surfaces", () => {
  const shellSurfaces = {
    shells: [{ id: "sh-1", description: "cargo test", state: "running", tail: ["running 12 tests"] }],
  };

  const openPanelWithSurfaces = async () => {
    payload = branchRow({ agents: [agent({ surfaces: shellSurfaces })] });
    await mount();
  };

  it("mounts the pills between the status line and the box", async () => {
    await openPanelWithSurfaces();

    const block = railHost().querySelector(".rail-composer");
    expect([...block.children].map((child) => child.id)).toEqual(["rail-status", "rail-surfaces", ""]);
    expect(block.querySelector('#rail-surfaces [data-surface-kind="shells"]')).not.toBe(null);
    expect(block.lastElementChild.querySelector("#railinput")).not.toBe(null);
  });

  it("sends a row's action as a message, leaving the draft and the focus alone", async () => {
    await openPanelWithSurfaces();
    const input = railHost().querySelector("#railinput");
    input.value = "half a sentence";
    input.focus();

    railHost().querySelector('[data-surface-kind="shells"]').click();
    const row = railHost().querySelector(".surface-shells .surface-row");
    row.querySelector(".caret").click();
    row.querySelector('.mi[data-action="stop-shell"]').click();
    await flush();

    expect(callsTo("thread.post")[0].params.body).toBe('Please stop the background command "cargo test".');
    expect(railHost().querySelector("#railinput").value).toBe("half a sentence");
    expect(document.activeElement).toBe(input);
    expect(railHost().querySelector(".surface-shells")).not.toBe(null);
  });
});
