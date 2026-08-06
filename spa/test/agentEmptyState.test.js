// @vitest-environment jsdom
// What the Agent tab says when there is no agent to look at.
//
// Two different silences, and conflating them is the bug this pins. A worktree
// nothing has run in yet is INERT — the honest offer is "start one", and
// starting one is a choice of harness, so the offer is the provider picker
// itself. A worktree whose harness exited (codex ran its self-update and quit,
// claude crashed) has a last screen worth reading, and the offer is "start it
// again" on the provider it already runs — laid OVER that screen rather than
// replacing it, because what it printed on the way out is usually why you are
// looking.
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
const cards = (el) => [...el.querySelectorAll("#agentStartChoices .chooser-card")];
const card = (el, provider) => cards(el).find((c) => c.dataset.provider === provider);
const cardLabel = (el, provider) => card(el, provider).querySelector(".chooser-card-label").textContent;

beforeEach(() => {
  document.body.innerHTML = "";
  attachAgent.mockReset();
  paneSpy.lastOpts = null;
});

describe("a worktree whose agent has never run", () => {
  beforeEach(() => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-1", live: false, snapshot: "", cursor: 0 });
  });

  it("says so in the middle of the pane and offers a harness to start", async () => {
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "wt-1" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toBe("No agent is currently running");
    // Which harness runs here has no answer yet, so the offer IS the question.
    expect(cards(el).map((c) => c.dataset.provider)).toEqual(["claude", "codex"]);
    expect(cards(el).map((c) => c.querySelector(".chooser-card-label").textContent)).toEqual(["Claude Code", "Codex"]);
    // Real buttons: reachable, pressable and labelled without a pointer.
    expect(cards(el).every((c) => c.tagName)).toBe(true);
    expect(cards(el).map((c) => c.tagName)).toEqual(["BUTTON", "BUTTON"]);
    expect(button(el).hidden).toBe(true);
    // Centered, not a pill pinned to the top edge.
    expect(overlay(el).className).toContain("agent-overlay");
    expect(overlay(el).classList.contains("over-screen")).toBe(false);
  });

  it("marks the harness already chosen for this worktree, without deciding for the human", async () => {
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "wt-1" }, { onStart: vi.fn(), selectedProvider: "codex" });
    await tick();

    expect(card(el, "codex").classList.contains("chosen")).toBe(true);
    expect(card(el, "claude").classList.contains("chosen")).toBe(false);
  });

  it("mounting still starts nothing — the offer is the whole point", async () => {
    const onStart = vi.fn();
    mountAgentTab(host(), { project_id: "p1", worktree_id: "wt-1" }, { onStart });
    await tick();
    expect(onStart).not.toHaveBeenCalled();
    expect(fakeManager.createTerminal).not.toHaveBeenCalled();
  });

  it("starts the harness the card names, and says so while it is coming up", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    card(el, "codex").click();
    await tick();
    expect(onStart).toHaveBeenCalledWith("codex");
    // Both cards go inert: a second press while the first is in flight would
    // start a race between two harnesses over one worktree.
    expect(cards(el).map((c) => c.disabled)).toEqual([true, true]);
    expect(cardLabel(el, "codex")).toBe("Starting…");

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  it("starts claude from its own card", async () => {
    const onStart = vi.fn(async () => {});
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    card(el, "claude").click();
    await tick();
    expect(onStart).toHaveBeenCalledWith("claude");
  });

  it("keeps the offer standing when the start fails, and says why", async () => {
    const onStart = vi.fn(async () => {
      throw new Error("no worktree to adopt");
    });
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    card(el, "codex").click();
    await tick();
    await tick();
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toContain("no worktree to adopt");
    expect(cards(el).map((c) => c.disabled)).toEqual([false, false]);
    expect(cardLabel(el, "codex")).toBe("Codex");
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
    expect(button(el).hidden).toBe(false);
    expect(button(el).textContent).toBe("Restart agent");
    // A worktree that has run one already HAS a harness — restarting it is not
    // a fresh choice, so the picker stays out of the way.
    expect(el.querySelector("#agentStartChoices").hidden).toBe(true);
    // The retained screen is still there underneath — that is the diagnosis.
    expect(overlay(el).classList.contains("over-screen")).toBe(true);
    expect(el.querySelector("#agentpane")).not.toBeNull();
  });

  it("restarts on the provider the entity already holds — it names none", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-2", live: false, snapshot: "exit 1\n", cursor: 6 });
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart });
    await tick();

    button(el).click();
    await tick();
    expect(onStart.mock.calls[0][0]).toBeUndefined();
    expect(button(el).disabled).toBe(true);
    expect(button(el).textContent).toBe("Starting…");

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
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
    expect(el.querySelector("#agentStartChoices")).toBeNull();
  });

  it("still stands the offer up when the attach itself fails", async () => {
    attachAgent.mockRejectedValueOnce(new Error("unknown worktree_id"));
    const el = host();
    mountAgentTab(el, { project_id: "p1", worktree_id: "gone" }, { onStart: vi.fn() });
    await tick();
    expect(overlay(el).hidden).toBe(false);
  });
});
