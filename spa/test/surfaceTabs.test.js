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

import { mountAuxTab, mountAgentPane, terminalTabsController } from "../src/core/surfaceTabs.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

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
    ...extra,
  };
}

function fakeHost() {
  const paneHost = fakeEl();
  const host = fakeEl({ querySelector: () => paneHost, paneHost });
  return host;
}

beforeEach(() => {
  fakeManager.listTerminals.mockReset();
  fakeManager.createTerminal.mockReset();
  fakeManager.closeTerminal.mockReset();
  fakeManager.attachTerminal.mockReset();
  fakeManager.detach.mockReset();
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
    mountAgentPane(host, "task-1", { onLive: () => {}, onExit: () => {} });
    await tick();
    expect(typeof paneSpy.lastOpts.onInputError).toBe("function");
  });

  it("routes an input RPC rejection to onExit('agent_session_ended') so the idle chip shows", async () => {
    const host = fakeHost();
    const exits = [];
    mountAgentPane(host, "task-1", { onLive: () => {}, onExit: (r) => exits.push(r) });
    await tick();
    // a keystroke hitting a dead session: term.input rejected → onInputError fires
    paneSpy.lastOpts.onInputError(new Error("no active agent session"));
    expect(exits).toEqual(["agent_session_ended"]);
  });
});
