// @vitest-environment jsdom
// What the TUI pane says when there is no session to look at.
//
// Two different silences, and conflating them is the bug this pins. A worktree
// nothing has run in yet is INERT, and a worktree whose harness exited (codex
// ran its self-update and quit, claude crashed) has a last screen worth reading
// — so the MESSAGE differs, and the exited one is laid OVER that screen rather
// than replacing it, because what it printed on the way out is usually why you
// are looking.
//
// The OFFER is one button in both: Resume. The pane belongs to an agent that
// already exists, and an agent is locked to the harness it was created on — so
// there is nothing left to choose here. Creating agents is the chat tab's job.
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
const resume = (el) => el.querySelector("#agentResume");

beforeEach(() => {
  document.body.innerHTML = "";
  attachAgent.mockReset();
  paneSpy.lastOpts = null;
});

describe("a worktree whose agent has never run", () => {
  beforeEach(() => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-1", live: false, snapshot: "", cursor: 0 });
  });

  it("says so in the middle of the pane and offers to resume the agent", async () => {
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "wt-1" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("No agent is currently running");
    // One button, because the agent whose pane this is already has its harness:
    // it was created on one and stays there for its whole life.
    expect(resume(el).tagName).toBe("BUTTON");
    expect(resume(el).textContent).toBe("Resume");
    expect(el.querySelectorAll(".chooser-card")).toHaveLength(0);
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

  it("resumes without naming a harness, and says so while it comes up", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    resume(el).click();
    await tick();
    // No provider at all: the agent is locked to its harness, so a resume that
    // named one could only name the one it is already on.
    expect(onStart).toHaveBeenCalledWith();
    expect(resume(el).disabled).toBe(true);
    expect(resume(el).textContent).toBe("Resuming…");

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  // The overlay re-renders on every state the socket reports, and a flapping
  // session can report one while a press is still in flight. The button the
  // human is watching must not be swapped for a fresh, idle one.
  it("keeps the button busy through a state report that lands mid-resume", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    resume(el).click();
    await tick();
    expect(resume(el).textContent).toBe("Resuming…");

    paneSpy.lastOpts.onExit("agent_session_ended"); // the old session dies under the press

    expect(resume(el).textContent).toBe("Resuming…");
    expect(resume(el).disabled).toBe(true);

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  it("keeps the offer standing when the resume fails, and says why", async () => {
    const onStart = vi.fn(async () => {
      throw new Error("no worktree to adopt");
    });
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    resume(el).click();
    await tick();
    await tick();
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toContain("no worktree to adopt");
    expect(resume(el).disabled).toBe(false);
    expect(resume(el).textContent).toBe("Resume");
  });
});

describe("an agent that exited", () => {
  beforeEach(() => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-2",
      live: false,
      provider: "codex",
      snapshot: "codex update installed\n",
      cursor: 41,
    });
  });

  it("overlays the screen it left behind rather than hiding it", async () => {
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    // No message: the retained screen behind the overlay already says what
    // happened, and the one button is the whole offer.
    expect(message(el)).toBe("");
    expect(el.querySelector("#agentOverlayMsg").hidden).toBe(true);
    expect(el.querySelector("#agentStartOffer").hidden).toBe(false);
    expect(resume(el).textContent).toBe("Resume");
    // Nothing here asks which harness: this agent has one already.
    expect(el.textContent).not.toContain("Claude Code");
    expect(el.textContent).not.toContain("Codex");
    // The retained screen is still there underneath — that is the diagnosis.
    expect(overlay(el).classList.contains("over-screen")).toBe(true);
    expect(el.querySelector("#agentpane")).not.toBeNull();
  });

  it("resumes the conversation the screen belongs to", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart });
    await tick();

    resume(el).click();
    await tick();
    expect(onStart).toHaveBeenCalledWith();
    expect(resume(el).disabled).toBe(true);
    expect(resume(el).textContent).toBe("Resuming…");

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  it("flips to the exited offer when a live session dies under the human", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-3", live: true, provider: "codex", snapshot: "", cursor: 0 });
    const el = host();
    mountAgentTab(el, { id: "run-3" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(true);

    paneSpy.lastOpts.onExit("agent_session_ended");
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("");
    expect(resume(el).textContent).toBe("Resume");
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
    expect(resume(el)).toBeNull();
    expect(el.querySelector("#agentStartOffer")).toBeNull();
  });

  it("still stands the offer up when the attach itself fails", async () => {
    attachAgent.mockRejectedValueOnce(new Error("unknown worktree_id"));
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "gone" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(false);
  });
});
