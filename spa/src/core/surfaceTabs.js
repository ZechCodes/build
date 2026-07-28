// Shared terminal/agent/files tab machinery for the three worktree-backed
// surfaces (task, external worktree, primary "main" checkout). The tab-row
// rendering is tabshell.js; this owns the DYNAMIC tab set (user terminals) and
// the mounting of a terminal/agent/files body — identical behavior everywhere
// (§7.2). Panes ride the ONE shared terminal socket (manager.js), demuxed by
// term_id; fs.* ride the app RPC session.

import { terminalManager, subscribeTerminalStatus } from "../terminal/manager.js";
import { mountTerminalPane } from "../terminal/pane.js";
import { renderFilesTab } from "../views/files.js";
import { esc } from "./text.js";

/**
 * Overlay a `.termpane` host with a connectivity chip + dim-while-offline, driven
 * by the shared terminal socket's status (subscribeTerminalStatus). A frozen
 * pane now says why. Same visual family as the agent-idle chip (a small pill
 * floated over the screen). Returns { dispose() } — unsubscribe + remove the chip.
 * Call AFTER the pane has mounted, so ghostty's own innerHTML reset (pane.js)
 * doesn't wipe the chip.
 */
export function attachConnectionOverlay(host) {
  const chip = document.createElement("div");
  chip.className = "term-conn";
  chip.hidden = true;
  chip.textContent = "reconnecting to your machine…";
  host.appendChild(chip);
  const unsubscribe = subscribeTerminalStatus((status) => {
    const offline = status !== "connected";
    chip.hidden = !offline;
    host.classList.toggle("term-offline", offline);
  });
  return {
    dispose() {
      unsubscribe();
      chip.remove();
      host.classList.remove("term-offline");
    },
  };
}

/** The one Build-owned agent of a worktree, as a tab.
 *
 *  A FIXTURE on every worktree surface: never closable, never minted by the
 *  human, present whether or not an agent has ever run there. It is where every
 *  human→agent path lands, so it must always be somewhere you can look. */
export const AGENT_TAB = { id: "agent", label: "Agent" };

/** What the tab row's `+` can open: the user's login shell, and nothing else.
 *
 *  A worktree has exactly one agent, Build owns it, and it lives in the Agent
 *  tab — so the `+` cannot mint a second one. It used to offer an unmanaged
 *  claude/codex session here: an agent with no `done` tool, no owner and no
 *  lifecycle, running in the same directory as the real one. */
export const NEW_TAB_KINDS = [
  { id: "shell", label: "Terminal", description: "your login shell in this directory" },
];

/** Track a surface's open user terminals: list on mount, create from the `+`,
 *  close on `×`. Labels are ordinals over list/creation order. */
export function terminalTabsController(scope) {
  const manager = terminalManager();
  let terms = []; // [{ term_id }] in list/creation order
  const labelOf = (termId) => {
    const index = terms.findIndex((t) => t.term_id === termId);
    return index < 0 ? "" : `Terminal ${index + 1}`;
  };
  return {
    ids: () => terms.map((t) => t.term_id),
    /** The tab descriptors for the shell: closable, ordinal-labeled. */
    tabs: () => terms.map((t) => ({ id: t.term_id, label: labelOf(t.term_id), closable: true })),
    label: labelOf,
    has: (termId) => terms.some((t) => t.term_id === termId),
    async load() {
      try {
        terms = (await manager.listTerminals(scope)).map((t) => ({ term_id: t.term_id }));
      } catch {
        terms = []; // an unknown/unresolvable scope means "no terminals"
      }
      return terms;
    },
    /** Open one of the user's shells in this surface's directory. */
    async create() {
      const r = await manager.createTerminal(scope, 80, 24);
      terms.push({ term_id: r.term_id });
      return r.term_id;
    },
    async close(termId) {
      try {
        await manager.closeTerminal(termId);
      } finally {
        terms = terms.filter((t) => t.term_id !== termId);
      }
    },
    /** Drop a terminal locally (it exited/was reaped server-side already). */
    drop(termId) {
      terms = terms.filter((t) => t.term_id !== termId);
    },
  };
}

/** Mount a user-terminal pane bound to `termId` on the shared socket. */
export function mountUserTerminalPane(host, termId, { onExit }) {
  const manager = terminalManager();
  return mountTerminalPane(host, {
    attach: (opts) => manager.attachTerminal(termId, opts),
    input: (data) => manager.input(termId, data),
    resize: (cols, rows) => manager.resize(termId, cols, rows),
    onExit,
  });
}

/** Mount a worktree's agent pane.
 *
 *  `target` is how the calling surface addresses that worktree: `{ id }` for a
 *  run or plan, or the scope of the directory itself (`{ run_id }`,
 *  `{ project_id, worktree_id }`, `{ project_id }`). The WIRE id comes back from
 *  the attach — it is `agent:<worktree_id>`, a hash of the canonical root that
 *  no client can compute — and every keystroke, resize and detach after that
 *  uses it. `onLive(bool)` reports session liveness (for the quiet idle chip);
 *  `onExit(reason)` reacts to closures. Input is allowed — a live PTY on the
 *  user's machine. */
export function mountAgentPane(host, target, { onLive, onExit }) {
  const manager = terminalManager();
  let termId = null; // learned from the attach; null until then, and after a failed one
  return mountTerminalPane(host, {
    attach: (opts) =>
      manager.attachAgent(target, { ...opts, onLive }).then((r) => {
        termId = r.term_id;
        return r;
      }),
    // Before the attach lands there is no screen to type into. Rejecting (rather
    // than dropping) routes through onInputError, so the idle chip shows instead
    // of the keystroke silently vanishing.
    input: (data) => (termId ? manager.input(termId, data) : Promise.reject(new Error("no active agent session"))),
    resize: (cols, rows) => (termId ? manager.resize(termId, cols, rows) : Promise.resolve()),
    onExit,
    // A keystroke rejected by the bridge means the session is gone — surface it
    // as the idle state (the same reason a live session's close reports), so the
    // "no active agent session" chip shows instead of the input silently
    // vanishing. Non-destructive: a new session's first frame clears it (B1).
    onInputError: () => onExit && onExit("agent_session_ended"),
  }).then((pane) => {
    // Overlay the connectivity chip once the pane exists (its mount resets the
    // host's innerHTML). Fold its teardown into the pane's own dispose.
    const conn = attachConnectionOverlay(host);
    return {
      ...pane,
      dispose() {
        conn.dispose();
        pane.dispose();
        // Deregister the screen the attach named — the server PTY keeps running.
        if (termId) manager.detach(termId);
      },
    };
  });
}

/** Mount the Agent tab's BODY for a worktree: its one agent as a full PTY, with
 *  a quiet chip whenever no session is live.
 *
 *  The same body on every worktree surface (task, external worktree, primary
 *  checkout, plan). Mounting it starts NOTHING: a worktree no agent has run in
 *  attaches to an empty screen and shows `idleLabel`. An agent begins when a
 *  human→agent verb delivers a turn, never because a tab was opened.
 *
 *  Returns { dispose() } — tears down the client view only. */
export function mountAgentTab(host, target, { idleLabel = "no active agent session" } = {}) {
  host.innerHTML = `<div class="agentwrap"><div class="agent-idle" id="agentIdle" hidden></div><div class="termpane" id="agentpane"></div></div>`;
  const chip = host.querySelector("#agentIdle");
  const setIdle = (on) => {
    if (!chip) return;
    chip.textContent = on ? idleLabel : "";
    chip.hidden = !on;
  };
  let pane = null;
  let disposed = false;
  mountAgentPane(host.querySelector("#agentpane"), target, {
    onLive: (live) => setIdle(!live),
    onExit: (reason) => {
      if (reason === "agent_session_ended") setIdle(true);
    },
  }).then(
    (p) => (disposed ? p.dispose() : (pane = p)),
    // An unresolvable address (a plan whose worktree is gone, a deleted run) is
    // the empty state too — the chip alone, the surface intact.
    () => setIdle(true),
  );
  return {
    dispose() {
      disposed = true;
      if (pane) pane.dispose();
    },
  };
}

/**
 * Mount the body for an auxiliary tab (files or a `term-<n>` user terminal) into
 * `host`. Returns { dispose() } — dispose tears down the CLIENT view only
 * (files: no-op; a pane: dispose + detach, never closing the server PTY). Agent
 * tabs are surface-specific (task only) and mounted by the task view directly.
 */
export function mountAuxTab(host, tabId, { scope, callRpc, onExit }) {
  if (tabId === "files") {
    renderFilesTab(host, { scope, callRpc });
    return { dispose() {} };
  }
  host.innerHTML = `<div class="termpane" id="termpane"></div>`;
  const paneHost = host.querySelector("#termpane");
  const manager = terminalManager();
  let pane = null;
  let conn = null;
  let disposed = false;
  mountUserTerminalPane(paneHost, tabId, { onExit }).then(
    (p) => {
      if (disposed) {
        p.dispose();
        return;
      }
      pane = p;
      conn = attachConnectionOverlay(paneHost);
    },
    (e) => {
      if (disposed) return;
      // "unknown term_id" = the terminal is gone (exited/closed elsewhere/reaped
      // while this tab was unmounted). §7.2: drop the tab — never a blank pane
      // that fails identically on every click. Other failures stay visible.
      if (/unknown term_id/.test((e && e.message) || "")) onExit("reaped");
      else paneHost.innerHTML = `<div class="empty">terminal unavailable: ${esc((e && e.message) || "error")}</div>`;
    },
  );
  return {
    dispose() {
      disposed = true;
      if (conn) conn.dispose();
      if (pane) pane.dispose();
      manager.detach(tabId);
    },
  };
}
