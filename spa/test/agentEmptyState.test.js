// @vitest-environment jsdom
// What the Agent tab says when there is no agent to look at.
//
// Two different silences, and conflating them is the bug this pins. A worktree
// nothing has run in yet is INERT — the honest offer is "start one". A worktree
// whose harness exited (codex ran its self-update and quit, claude crashed) has
// a last screen worth reading, and the offer is "start it again" — laid OVER
// that screen rather than replacing it, because what it printed on the way out
// is usually why you are looking.
//
// Both were a single 10px pill reading "no active agent session", which told
// you neither which case you were in nor what to do about it.

import { describe, it, expect, vi, beforeEach } from "vitest";

// The real manager reports liveness out of the attach response (session.js
// `_applyAttachResult`), which is what tells "never ran" from "ran and stopped".
// A double that only resolves would never exercise either.
const attachAgent = vi.fn();
const fakeManager = {
  attachAgent: (target, opts) =>
    attachAgent(target, opts).then((r) => {
      if (opts && opts.onLive) opts.onLive(!!r.live, r);
      return r;
    }),
  input: vi.fn(async () => {}),
  resize: vi.fn(async () => {}),
  detach: vi.fn(),
  listTerminals: vi.fn(),
  createTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  attachTerminal: vi.fn(),
};
const paneSpy = vi.hoisted(() => ({ lastOpts: null }));
vi.mock("../src/terminal/manager.js", () => ({
  terminalManager: () => fakeManager,
  subscribeTerminalStatus: () => () => {},
}));
vi.mock("../src/terminal/pane.js", () => ({
  mountTerminalPane: async (host, opts) => {
    paneSpy.lastOpts = opts;
    await opts.attach({ cols: 80, rows: 24, onSnapshot: () => {}, onOutput: () => {}, onClosed: () => {} });
    return { dispose: vi.fn() };
  },
}));

import { mountAgentTab } from "../src/core/surfaceTabs.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const host = () => document.body.appendChild(document.createElement("div"));
const overlay = (el) => el.querySelector("#agentOverlay");
const message = (el) => el.querySelector("#agentOverlayMsg").textContent;
const button = (el) => el.querySelector("#agentStart");

beforeEach(() => {
  document.body.innerHTML = "";
  attachAgent.mockReset();
  paneSpy.lastOpts = null;
});

describe("a worktree whose agent has never run", () => {
  beforeEach(() => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-1", live: false, snapshot: "", cursor: 0 });
  });

  it("says so in the middle of the pane and offers to start one", async () => {
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "wt-1" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("No agent is currently running");
    expect(button(el).textContent).toBe("Start agent");
    // Centered, not a pill pinned to the top edge.
    expect(overlay(el).className).toContain("agent-overlay");
    expect(overlay(el).classList.contains("over-screen")).toBe(false);
  });

  it("mounting still starts nothing — the offer is the whole point", async () => {
    const onStart = vi.fn();
    mountAgentTab(host(), { project_id: "p1", worktree_id: "wt-1" }, { onStart });
    await tick();
    expect(onStart).not.toHaveBeenCalled();
    expect(fakeManager.createTerminal).not.toHaveBeenCalled();
  });

  it("starts one when the button is pressed, and says so while it is coming up", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    button(el).click();
    await tick();
    expect(onStart).toHaveBeenCalledTimes(1);
    expect(button(el).disabled).toBe(true);
    expect(button(el).textContent).toBe("Starting…");

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  it("keeps the offer standing when the start fails, and says why", async () => {
    const onStart = vi.fn(async () => {
      throw new Error("no worktree to adopt");
    });
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    button(el).click();
    await tick();
    await tick();
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toContain("no worktree to adopt");
    expect(button(el).disabled).toBe(false);
    expect(button(el).textContent).toBe("Start agent");
  });
});

describe("an agent that exited", () => {
  it("overlays the screen it left behind rather than hiding it", async () => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-2",
      live: false,
      snapshot: "codex update installed\n",
      cursor: 41,
    });
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("The agent exited");
    expect(button(el).textContent).toBe("Restart agent");
    // The retained screen is still there underneath — that is the diagnosis.
    expect(overlay(el).classList.contains("over-screen")).toBe(true);
    expect(el.querySelector("#agentpane")).not.toBeNull();
  });

  it("flips to the exited offer when a live session dies under the human", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-3", live: true, snapshot: "", cursor: 0 });
    const el = host();
    mountAgentTab(el, { id: "run-3" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(true);

    paneSpy.lastOpts.onExit("agent_session_ended");
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("The agent exited");
    expect(button(el).textContent).toBe("Restart agent");
  });
});

describe("a live agent", () => {
  it("shows no overlay at all — the terminal is the whole tab", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-4", live: true, snapshot: "$ ", cursor: 2 });
    const el = host();
    mountAgentTab(el, { id: "run-4" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });
});

describe("a surface with nothing that could own an agent", () => {
  it("explains itself instead of offering a button that cannot work", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-5", live: false, snapshot: "", cursor: 0 });
    const el = host();
    mountAgentTab(el, { project_id: "p1" }, {}); // no onStart
    await tick();
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("No agent is currently running");
    expect(button(el)).toBeNull();
  });

  it("still stands the offer up when the attach itself fails", async () => {
    attachAgent.mockRejectedValueOnce(new Error("unknown worktree_id"));
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "gone" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(false);
  });
});
