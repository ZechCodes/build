// @vitest-environment jsdom
// What the Agent tab says when there is no agent to look at.
//
// Two different silences, and conflating them is the bug this pins. A worktree
// nothing has run in yet is INERT, and a worktree whose harness exited (codex
// ran its self-update and quit, claude crashed) has a last screen worth reading
// — so the MESSAGE differs, and the exited one is laid OVER that screen rather
// than replacing it, because what it printed on the way out is usually why you
// are looking.
//
// The OFFER is the same in both: the harness this worktree already ran (or the
// default, where none has) as a wide button, with both providers under it. A
// restart is a start; the only thing exiting changed is which provider is the
// obvious one, and that is a label, not a different control.
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
const lead = (el) => el.querySelector("#agentStartLead");
const labelOf = (el) => el.querySelector(".chooser-card-label").textContent;
const cards = (el) => [...el.querySelectorAll("#agentStartChoices .chooser-cards .chooser-card")];
const card = (el, provider) => cards(el).find((c) => c.dataset.provider === provider);
const cardLabel = (el, provider) => labelOf(card(el, provider));

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
    expect(cards(el).map(labelOf)).toEqual(["Claude Code", "Codex"]);
    // Real buttons: reachable, pressable and labelled without a pointer.
    expect(cards(el).every((c) => c.tagName)).toBe(true);
    expect(cards(el).map((c) => c.tagName)).toEqual(["BUTTON", "BUTTON"]);
    // With nothing to go on, the wide button leads with the default — still a
    // named harness, so pressing it is never a mystery.
    expect(lead(el).tagName).toBe("BUTTON");
    expect(lead(el).dataset.provider).toBe("claude");
    expect(labelOf(lead(el))).toBe("Claude Code");
    expect(lead(el).textContent).toContain("the default");
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
    // Every control goes inert: a second press while the first is in flight
    // would start a race between two harnesses over one worktree.
    expect(cards(el).map((c) => c.disabled)).toEqual([true, true]);
    expect(lead(el).disabled).toBe(true);
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

  it("starts the harness the wide button names, naming it explicitly", async () => {
    const onStart = vi.fn(async () => {});
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    lead(el).click();
    await tick();
    // Never an unnamed start: the button says which harness it runs, so the
    // call says the same thing.
    expect(onStart).toHaveBeenCalledWith("claude");
    expect(labelOf(lead(el))).toBe("Starting…");
  });

  // The overlay re-renders on every state the socket reports, and a flapping
  // session can report one while a press is still in flight. The card the human
  // is watching must not be swapped for a fresh, idle one.
  it("keeps the pressed card busy through a state report that lands mid-start", async () => {
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-1" }, { onStart });
    await tick();

    card(el, "codex").click();
    await tick();
    expect(cardLabel(el, "codex")).toBe("Starting…");

    paneSpy.lastOpts.onExit("agent_session_ended"); // the old session dies under the press

    expect(cardLabel(el, "codex")).toBe("Starting…");
    expect(cards(el).map((c) => c.disabled)).toEqual([true, true]);

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

    card(el, "codex").click();
    await tick();
    await tick();
    expect(overlay(el).hidden).toBe(false);
    expect(message(el)).toContain("no worktree to adopt");
    expect(cards(el).map((c) => c.disabled)).toEqual([false, false]);
    expect(cardLabel(el, "codex")).toBe("Codex");
    expect(lead(el).disabled).toBe(false);
    expect(labelOf(lead(el))).toBe("Claude Code");
  });
});

describe("an agent that exited", () => {
  it("overlays the screen it left behind rather than hiding it", async () => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-2",
      live: false,
      provider: "codex",
      snapshot: "codex update installed\n",
      cursor: 41,
    });
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart: vi.fn() });
    await tick();

    expect(overlay(el).hidden).toBe(false);
    // No message: the retained screen behind the overlay already says what
    // happened, and the picker is the whole offer.
    expect(message(el)).toBe("");
    expect(el.querySelector("#agentOverlayMsg").hidden).toBe(true);
    // The same picker as the idle state — a restart IS a start. What exiting
    // changed is only which harness is the obvious one, and the attach says
    // which one painted the screen behind this overlay.
    expect(el.querySelector("#agentStartChoices").hidden).toBe(false);
    expect(lead(el).dataset.provider).toBe("codex");
    expect(labelOf(lead(el))).toBe("Codex");
    expect(lead(el).textContent).toContain("ran here last");
    expect(cards(el).map((c) => c.dataset.provider)).toEqual(["claude", "codex"]);
    // No unnamed "Restart agent": every offer here says what it will run.
    expect(el.querySelector("#agentStart")).toBeNull();
    expect(el.textContent).not.toContain("Restart");
    // The retained screen is still there underneath — that is the diagnosis.
    expect(overlay(el).classList.contains("over-screen")).toBe(true);
    expect(el.querySelector("#agentpane")).not.toBeNull();
  });

  it("starts the harness that painted the screen, and names it", async () => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-2",
      live: false,
      provider: "codex",
      snapshot: "exit 1\n",
      cursor: 6,
    });
    let release;
    const onStart = vi.fn(() => new Promise((resolve) => (release = resolve)));
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart });
    await tick();

    lead(el).click();
    await tick();
    expect(onStart).toHaveBeenCalledWith("codex");
    expect(lead(el).disabled).toBe(true);
    expect(labelOf(lead(el))).toBe("Starting…");
    expect(cards(el).map((c) => c.disabled)).toEqual([true, true]);

    release();
    await tick();
    expect(overlay(el).hidden).toBe(true);
  });

  it("can switch harness on the way back up — the cards are still there", async () => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-2",
      live: false,
      provider: "codex",
      snapshot: "exit 1\n",
      cursor: 6,
    });
    const onStart = vi.fn(async () => {});
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart });
    await tick();

    card(el, "claude").click();
    await tick();
    expect(onStart).toHaveBeenCalledWith("claude");
  });

  // The bridge records which carrier a session actually opened, and that record
  // outlives the account setting that chose it. The offer names the agent the
  // human knows and restarts exactly what ran — the carrier stays the bridge's
  // business, never a second Claude Code on the picker.
  it("names a claude carrier the picker does not list as Claude Code, and restarts it", async () => {
    attachAgent.mockResolvedValue({
      term_id: "agent:wt-4",
      live: false,
      provider: "claude_adk",
      snapshot: "session ended\n",
      cursor: 14,
    });
    const onStart = vi.fn(async () => {});
    const el = host();
    mountAgentTab(el, { id: "run-4" }, { onStart });
    await tick();

    expect(labelOf(lead(el))).toBe("Claude Code");
    expect(lead(el).textContent).toContain("ran here last");
    expect(cards(el).map(labelOf)).toEqual(["Claude Code", "Codex"]);
    lead(el).click();
    await tick();
    expect(onStart).toHaveBeenCalledWith("claude_adk");
  });

  it("leads with the default when the bridge names no harness", async () => {
    attachAgent.mockResolvedValue({ term_id: "agent:wt-2", live: false, snapshot: "gone\n", cursor: 5 });
    const el = host();
    mountAgentTab(el, { id: "run-2" }, { onStart: vi.fn() });
    await tick();

    expect(message(el)).toBe("");
    expect(lead(el).dataset.provider).toBe("claude");
    expect(lead(el).textContent).toContain("the default");
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
    // The session that just died is what this worktree ran — the offer leads
    // with it even though the exit itself carries no payload.
    expect(lead(el).dataset.provider).toBe("codex");
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
    expect(lead(el)).toBeNull();
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
