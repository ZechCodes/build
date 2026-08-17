// @vitest-environment jsdom
// The agent rail's wiring: the strip that is always there, the panel that
// expands beside it, the two faces of an agent, and the first message — which
// on a checkout Build owns nothing in is what brings the agent into being.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

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

const { App } = await import("../src/app.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { createAgentSelection } = await import("../src/core/agentSelection.js");
const { createAdoptingCall } = await import("../src/core/adoption.js");

const agent = (over = {}) => ({
  id: "ag-1", ordinal: 1, provider: "claude", state: "live",
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

const railHost = () => document.getElementById("agent-rail");
const bubbles = () => [...railHost().querySelectorAll(".rail-bubble")];
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

beforeEach(() => {
  document.body.innerHTML = bodyHtml;
  localStorage.clear();
  resetAgentRailMemory();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  calls = [];
  payload = branchRow();
  feedSubscribers.clear();
  feedSnapshot = { items: [], projects: [] };
  markSeen.mockClear();
  mountAgentTab.mockClear();
  notifyError.mockClear();
  App.call = vi.fn(async (method, params) => {
    calls.push({ method, params });
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
    expect(bubbles()[1].querySelector(".rail-badge").textContent).toBe("4");
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

  it("repaints the strip the moment a bubble has something new to say", async () => {
    await mount();
    const before = bubbles()[0];
    payload = branchRow({ agents: [agent({ unread_count: 3 })] });

    vi.advanceTimersByTime(1600);
    await flush();

    expect(bubbles()[0]).not.toBe(before);
    expect(bubbles()[0].querySelector(".rail-badge").textContent).toBe("3");
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

  it("gives a branch another agent on the account's harness, and opens it", async () => {
    localStorage.setItem("build.agentDefaults", JSON.stringify({ provider: "codex", model: "", effort: "" }));
    await mount();
    railHost().querySelector('[data-bubble="add"]').click();
    await flush();
    expect(callsTo("agent.add")[0].params).toEqual({ entity_id: "run-3", provider: "codex" });
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

  it("offers removal on an agent added beside the first, and never on the first", async () => {
    payload = twoAgents();
    await mount();
    // The rail opens on the first agent, which owns the branch's conversation.
    expect(removeButton()).toBe(null);
    bubbles()[1].click();
    await flush();
    expect(removeButton()).toBeTruthy();
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
    expect(markSeen).toHaveBeenCalledWith("run-3", "ag-1");
  });

  it("says nothing about reading a conversation with nothing waiting", async () => {
    await mount();
    expect(markSeen).not.toHaveBeenCalled();
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

  it("says nothing twice to an agent already listening", async () => {
    await mount();
    panel().querySelector("#railinput").value = "hi";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("agent.start")).toEqual([]);
  });

  it("adopts a checkout Build owns nothing in, then speaks, then starts the agent it made", async () => {
    payload = branchRow({ run_id: null, run: null, agents: [] });
    await mount();
    panel().querySelector("#railinput").value = "start here";
    panel().querySelector("#railsend").click();
    await flush();
    expect(callsTo("run.adopt")[0].params).toMatchObject({ project_id: "p1", worktree_id: "wt-3" });
    expect(callsTo("thread.post")[0].params).toMatchObject({ entity_id: "run-9", body: "start here" });
    expect(callsTo("thread.post")[0].params.agent_id).toBeUndefined();
    expect(callsTo("agent.start")[0].params).toEqual({ id: "run-9" });
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
