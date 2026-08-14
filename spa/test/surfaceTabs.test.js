import { describe, it, expect, vi, beforeEach } from "vitest";

// Isolate the agent pane's wiring: the pane just forwards the manager's attach
// (no ghostty/wasm in node), and the manager is a test double. The user's own
// terminals live in the console now — their tests are consoleDom.test.js.
const fakeManager = {
  listTerminals: vi.fn(),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  attachAgent: vi.fn(async () => ({ live: true, snapshot: "", cursor: 0 })),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
// Capture the last opts the pane was mounted with, so a test can drive the
// pane's callbacks (onInputError) without a real ghostty terminal.
const paneSpy = vi.hoisted(() => ({ lastOpts: null }));
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => fakeManager,
  subscribeTerminalStatus: () => () => {}, // returns an unsubscribe
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    paneSpy.lastOpts = opts;
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

import { mountAgentPane, mountAgentTab } from "../src/core/surfaceTabs.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

let agentAttachResult; // what the bridge answers the next agent attach with

// attachConnectionOverlay appends a chip element to the pane host and toggles a
// class on it, so the host doubles gain the minimal DOM surface it touches.
global.document = global.document || {
  createElement: () => ({ className: "", hidden: true, textContent: "", remove() {} }),
};

function fakeEl(extra = {}) {
  return {
    innerHTML: "",
    classList: { toggle() {}, remove() {}, add() {} },
    appendChild() {},
    querySelectorAll: () => [], // the idle offer's provider cards, when it has any
    ...extra,
  };
}

function fakeHost() {
  const paneHost = fakeEl();
  const chip = fakeEl({ textContent: "", hidden: true });
  const host = fakeEl({
    querySelector: (selector) => (selector === "#agentIdle" ? chip : paneHost),
    paneHost,
    chip,
  });
  return host;
}

beforeEach(() => {
  fakeManager.listTerminals.mockReset();
  fakeManager.createTerminal.mockReset();
  fakeManager.closeTerminal.mockReset();
  fakeManager.attachTerminal.mockReset();
  fakeManager.detach.mockReset();
  fakeManager.input.mockReset();
  fakeManager.resize.mockReset();
  fakeManager.attachAgent.mockReset();
  // The bridge answers every agent attach with the WORKTREE's wire id, and
  // reports liveness through the registered onLive (which is how the idle chip
  // learns there is no session).
  agentAttachResult = { term_id: "agent:wt-3", live: true, snapshot: "", cursor: 0 };
  fakeManager.attachAgent.mockImplementation(async (target, opts) => {
    if (opts && opts.onLive) opts.onLive(!!agentAttachResult.live);
    return agentAttachResult;
  });
  paneSpy.lastOpts = null;
});

describe("mountAgentPane surfaces a dropped input error (S3, pairs with B1)", () => {
  it("provides an onInputError so a rejected keystroke is not swallowed", async () => {
    const host = fakeHost();
    mountAgentPane(host, { id: "task-1" }, { onLive: () => {}, onExit: () => {} });
    await tick();
    expect(typeof paneSpy.lastOpts.onInputError).toBe("function");
  });

  it("routes an input RPC rejection to onExit('agent_session_ended') so the idle chip shows", async () => {
    const host = fakeHost();
    const exits = [];
    mountAgentPane(host, { id: "task-1" }, { onLive: () => {}, onExit: (r) => exits.push(r) });
    await tick();
    // a keystroke hitting a dead session: term.input rejected → onInputError fires
    paneSpy.lastOpts.onInputError(new Error("no active agent session"));
    expect(exits).toEqual(["agent_session_ended"]);
  });
});

// An agent belongs to a worktree, and a surface addresses that worktree the way
// it already knows it: a run/plan id, or a scope. The wire id comes BACK from
// the attach (it is a hash of the canonical root), and every keystroke, resize
// and detach after that must use it — not the address the surface asked with.
describe("mountAgentPane addresses a worktree and keys itself by the bridge's answer", () => {
  it("passes the surface's address straight through to agent.attach", async () => {
    const host = fakeHost();
    mountAgentPane(host, { project_id: "p1", worktree_id: "wt-3" }, { onLive: () => {}, onExit: () => {} });
    await tick();
    expect(fakeManager.attachAgent.mock.calls[0][0]).toEqual({ project_id: "p1", worktree_id: "wt-3" });
  });

  it("sends input and resize to the wire id the attach answered with", async () => {
    const host = fakeHost();
    await mountAgentPane(host, { project_id: "p1", worktree_id: "wt-3" }, { onLive: () => {}, onExit: () => {} });
    await paneSpy.lastOpts.input("hi");
    await paneSpy.lastOpts.resize(100, 30);
    expect(fakeManager.input).toHaveBeenCalledWith("agent:wt-3", "hi");
    expect(fakeManager.resize).toHaveBeenCalledWith("agent:wt-3", 100, 30);
  });

  it("lets go of that screen on dispose, so an unmounted tab stops streaming", async () => {
    const host = fakeHost();
    const pane = await mountAgentPane(host, { id: "run-2" }, { onLive: () => {}, onExit: () => {} });
    pane.dispose();
    expect(fakeManager.detach).toHaveBeenCalledWith("agent:wt-3");
  });
});

// The Agent tab is a FIXTURE on every worktree surface — mounting it must never
// spawn an agent. A worktree nothing has run in renders its empty state.
describe("mountAgentTab", () => {
  it("mounts a pane without asking anything to start", async () => {
    agentAttachResult = { term_id: "agent:wt-8", live: false, snapshot: "", cursor: 0 };
    mountAgentTab(fakeHost(), { project_id: "p1", worktree_id: "wt-8" }, { onStart: vi.fn() });
    await tick();
    expect(fakeManager.createTerminal).not.toHaveBeenCalled();
  });

  // What the tab SAYS when nothing is live — and the difference between a
  // worktree that never ran one and one whose agent exited — needs real
  // elements to assert (a centered block with a button, not a text node), so it
  // lives in agentEmptyState.test.js under jsdom rather than against this
  // file's hand-rolled host.
});
