// @vitest-environment jsdom
// The Agent tab survives the surface repainting under it.
//
// The task surface polls every 1.6 s and rebuilds its shell whenever the run
// moves — and that rebuild replaces #tabbody, throwing away whatever pane was
// mounted in it. Every tab that owns its own body has to be remounted on the
// other side, and the Agent tab is one of them: the agent is a live PTY the
// human is watching, so dropping it on a state change blanks the screen
// mid-session with nothing to click that brings it back.
//
// agentTab.test.js pins that the tab EXISTS on every surface; this pins that
// looking at it survives a repaint.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

// The pane rides the shared terminal socket in production; here the manager is
// a double, so mounting an agent pane is an attachAgent call and nothing else.
const fakeManager = {
  listTerminals: vi.fn(async () => []),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  attachTerminal: vi.fn(),
  attachAgent: vi.fn(async () => ({ term_id: "agent:worktree", live: true, snapshot: "", cursor: 0 })),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
};
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => fakeManager,
  subscribeTerminalStatus: () => () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

import { App } from "../src/app.js";
import { renderTask } from "../src/views/task.js";

const runPayload = (state) => ({
  run_id: "r-1",
  state,
  goal: "a goal",
  project_id: "p-1",
  branch: "build/thing",
  worktree_path: "/tmp/wt",
  stages: [],
  thread: { items: [] },
});

let runState;
const agentPane = () => document.querySelector("#tabbody #agentpane");
const attachCount = () => fakeManager.attachAgent.mock.calls.length;

describe("the Agent tab across a poll-driven repaint (DOM)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    document.body.innerHTML = '<div id="root"></div>';
    App.offline = false;
    App.poll = null;
    App.viewDispose = null;
    runState = "building";
    fakeManager.attachAgent.mockClear();
    App.call = vi.fn(async (method) => (method === "run.get" ? runPayload(runState) : {}));
  });
  afterEach(() => {
    if (App.poll) clearInterval(App.poll);
    App.poll = null;
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("mounts the agent pane on a surface opened straight onto it", async () => {
    App.route = { name: "task", id: "r-1", tab: "agent" };
    location.hash = "#/task/r-1/agent";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);

    expect(agentPane()).toBeTruthy();
    expect(attachCount()).toBeGreaterThan(0);
  });

  it("remounts it after the run moves and the shell is rebuilt", async () => {
    App.route = { name: "task", id: "r-1", tab: "agent" };
    location.hash = "#/task/r-1/agent";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);
    const paneBeforeMove = agentPane();
    const attachesBeforeMove = attachCount();
    expect(paneBeforeMove).toBeTruthy();

    // The run moves to review: the poll rebuilds the shell, which replaces the
    // body the pane was living in.
    runState = "review";
    await vi.advanceTimersByTimeAsync(1600);

    expect(paneBeforeMove.isConnected).toBe(false); // the rebuild threw it away
    expect(agentPane()).toBeTruthy(); // and the tab put a live one back
    expect(attachCount()).toBeGreaterThan(attachesBeforeMove);
    // Still the selected tab, so the human never had to re-pick it.
    expect(document.querySelector(".tabs .t.active").dataset.tab).toBe("agent");
  });

  it("leaves a repaint alone when the human is on another surface tab", async () => {
    App.route = { name: "task", id: "r-1", tab: "files" };
    location.hash = "#/task/r-1/files";
    await renderTask();
    await vi.advanceTimersByTimeAsync(0);

    const attachesOnFiles = attachCount();
    runState = "review";
    await vi.advanceTimersByTimeAsync(1600);
    expect(attachCount()).toBe(attachesOnFiles);
    expect(agentPane()).toBeNull();
  });
});
