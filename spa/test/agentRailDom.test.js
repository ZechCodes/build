// @vitest-environment jsdom
// The agent rail's wiring: the strip that is always there, the panel that
// expands beside it, the two faces of an agent, and the first message — which
// on a checkout Build owns nothing in is what brings the agent into being.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const refreshFeed = vi.fn(async () => {});
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: () => () => {},
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
  markSeen.mockClear();
  mountAgentTab.mockClear();
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
