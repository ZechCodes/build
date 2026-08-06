import { describe, it, expect, vi, beforeEach } from "vitest";

// Isolate mountAuxTab's failure handling: the pane just forwards the manager's
// attach rejection (no ghostty/wasm in node), and the manager is a test double.
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
vi.mock("../src/views/files.js", () => ({ renderFilesTab: vi.fn() }));

import { mountAuxTab, mountAgentPane, mountAgentTab, terminalTabsController, AGENT_TAB, NEW_TAB_KINDS } from "../src/core/surfaceTabs.js";

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

describe("terminalTabsController labels", () => {
  it("uses readable terminal names for listed and newly-created sessions", async () => {
    fakeManager.listTerminals.mockResolvedValue([{ term_id: "term-a" }, { term_id: "term-b" }]);
    fakeManager.createTerminal.mockResolvedValue({ term_id: "term-c" });
    const controller = terminalTabsController({ project_id: "p1" });
    await controller.load();
    expect(controller.tabs().map((tab) => tab.label)).toEqual(["Terminal 1", "Terminal 2"]);
    await controller.create();
    expect(controller.label("term-c")).toBe("Terminal 3");
  });

  // Every tab the human opens is a shell. The one agent of a worktree is
  // Build's, lives in the Agent tab, and is never one of these — a `+` that
  // could mint a claude session put a second, unmanaged agent in the same
  // directory as the real one.
  it("calls every user tab a Terminal, whatever a daemon reports about it", async () => {
    fakeManager.listTerminals.mockResolvedValue([
      { term_id: "term-a", kind: "shell" },
      { term_id: "term-b", kind: "claude" },
    ]);
    const controller = terminalTabsController({ project_id: "p1" });
    await controller.load();
    expect(controller.tabs().map((tab) => tab.label)).toEqual(["Terminal 1", "Terminal 2"]);
  });

  it("asks term.create for nothing but a shell in this surface's directory", async () => {
    fakeManager.listTerminals.mockResolvedValue([]);
    fakeManager.createTerminal.mockResolvedValue({ term_id: "term-x" });
    const controller = terminalTabsController({ run_id: "run-1" });
    await controller.load();
    await controller.create();
    expect(fakeManager.createTerminal).toHaveBeenCalledWith({ run_id: "run-1" }, 80, 24);
    expect(controller.label("term-x")).toBe("Terminal 1");
  });
});

describe("mountAuxTab attach failure (§7.2: a stale terminal tab must drop, not blank)", () => {
  it("an unknown term_id rejection reports onExit('reaped') so the tab is dropped", async () => {
    fakeManager.attachTerminal.mockRejectedValue(new Error("unknown term_id"));
    const host = fakeHost();
    const exits = [];
    mountAuxTab(host, "term-3", { scope: { run_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    await tick();
    expect(exits).toEqual(["reaped"]);
  });

  it("any other failure renders an error in the pane instead of a silent blank", async () => {
    fakeManager.attachTerminal.mockRejectedValue(new Error("rpc term.attach timeout"));
    const host = fakeHost();
    const exits = [];
    mountAuxTab(host, "term-3", { scope: { run_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    await tick();
    expect(exits).toEqual([]);
    expect(host.paneHost.innerHTML).toContain("timeout");
  });

  it("a failure after dispose stays quiet (no onExit for a tab already gone)", async () => {
    let rejectAttach;
    fakeManager.attachTerminal.mockReturnValue(new Promise((_, reject) => (rejectAttach = reject)));
    const host = fakeHost();
    const exits = [];
    const ctl = mountAuxTab(host, "term-3", { scope: { run_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    ctl.dispose();
    rejectAttach(new Error("unknown term_id"));
    await tick();
    expect(exits).toEqual([]);
  });
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
  it("is the same tab on every surface: never closable, never minted by the human", () => {
    expect(AGENT_TAB).toEqual({ id: "agent", label: "Agent" });
    expect(NEW_TAB_KINDS.map((kind) => kind.id)).toEqual(["shell"]);
  });

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
