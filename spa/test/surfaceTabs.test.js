import { describe, it, expect, vi, beforeEach } from "vitest";

// Isolate mountAuxTab's failure handling: the pane just forwards the manager's
// attach rejection (no ghostty/wasm in node), and the manager is a test double.
const fakeManager = {
  attachTerminal: vi.fn(),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
vi.mock("../src/terminal/manager.js", () => ({ terminalManager: () => fakeManager }));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, { attach }) => {
    await attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));
vi.mock("../src/views/files.js", () => ({ renderFilesTab: vi.fn() }));

import { mountAuxTab } from "../src/core/surfaceTabs.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function fakeHost() {
  const paneHost = { innerHTML: "" };
  return { innerHTML: "", querySelector: () => paneHost, paneHost };
}

beforeEach(() => {
  fakeManager.attachTerminal.mockReset();
  fakeManager.detach.mockReset();
});

describe("mountAuxTab attach failure (§7.2: a stale terminal tab must drop, not blank)", () => {
  it("an unknown term_id rejection reports onExit('reaped') so the tab is dropped", async () => {
    fakeManager.attachTerminal.mockRejectedValue(new Error("unknown term_id"));
    const host = fakeHost();
    const exits = [];
    mountAuxTab(host, "term-3", { scope: { task_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    await tick();
    expect(exits).toEqual(["reaped"]);
  });

  it("any other failure renders an error in the pane instead of a silent blank", async () => {
    fakeManager.attachTerminal.mockRejectedValue(new Error("rpc term.attach timeout"));
    const host = fakeHost();
    const exits = [];
    mountAuxTab(host, "term-3", { scope: { task_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    await tick();
    expect(exits).toEqual([]);
    expect(host.paneHost.innerHTML).toContain("timeout");
  });

  it("a failure after dispose stays quiet (no onExit for a tab already gone)", async () => {
    let rejectAttach;
    fakeManager.attachTerminal.mockReturnValue(new Promise((_, reject) => (rejectAttach = reject)));
    const host = fakeHost();
    const exits = [];
    const ctl = mountAuxTab(host, "term-3", { scope: { task_id: "t1" }, callRpc: async () => ({}), onExit: (r) => exits.push(r) });
    ctl.dispose();
    rejectAttach(new Error("unknown term_id"));
    await tick();
    expect(exits).toEqual([]);
  });
});
