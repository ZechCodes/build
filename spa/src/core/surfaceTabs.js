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

/** What the tab row's `+` can open, in menu order. A tab is a window onto the
 *  user's machine in this surface's directory — their shell, or a coding agent
 *  THEY drive. An agent tab is not a run: no plan, no task lifecycle, no `done`
 *  report; Build only spawns the CLI and streams the PTY. The daemon owns each
 *  kind's argv (`term.create` takes a kind, never a command line). */
export const NEW_TAB_KINDS = [
  { id: "shell", label: "Terminal", description: "your login shell in this directory" },
  { id: "claude", label: "Claude Code", description: "an interactive claude session you drive" },
  { id: "codex", label: "Codex", description: "an interactive codex session you drive" },
];

const KIND_LABELS = Object.fromEntries(NEW_TAB_KINDS.map((kind) => [kind.id, kind.label]));

/** The tab name for a terminal: what runs in it, numbered within its own kind
 *  ("Terminal 1" beside "Claude Code 1"). An unrecognized kind — a daemon newer
 *  than this client — labels itself rather than posing as a shell. */
function kindLabel(kind) {
  return KIND_LABELS[kind] || kind || "Terminal";
}

/** Track a surface's open user terminals: list on mount, create from the `+`
 *  menu, close on `×`. Labels are per-kind ordinals over list/creation order. */
export function terminalTabsController(scope) {
  const manager = terminalManager();
  let terms = []; // [{ term_id, kind }] in list/creation order
  const labelOf = (termId) => {
    const index = terms.findIndex((t) => t.term_id === termId);
    if (index < 0) return "";
    const { kind } = terms[index];
    const ordinal = terms.slice(0, index + 1).filter((t) => t.kind === kind).length;
    return `${kindLabel(kind)} ${ordinal}`;
  };
  return {
    ids: () => terms.map((t) => t.term_id),
    /** The tab descriptors for the shell: closable, kind-labeled. */
    tabs: () => terms.map((t) => ({ id: t.term_id, label: labelOf(t.term_id), closable: true })),
    label: labelOf,
    has: (termId) => terms.some((t) => t.term_id === termId),
    async load() {
      try {
        // A terminal without a reported kind is a shell — the same reading the
        // daemon gives an absent `kind`, and what every pre-menu terminal is.
        terms = (await manager.listTerminals(scope)).map((t) => ({ term_id: t.term_id, kind: t.kind || "shell" }));
      } catch {
        terms = []; // an unknown/unresolvable scope means "no terminals"
      }
      return terms;
    },
    /** Open a tab of `kind` (default: the user's shell). The daemon's echoed
     *  kind wins over the requested one — it is the one that actually spawned. */
    async create(kind = "shell") {
      const r = await manager.createTerminal(scope, 80, 24, kind);
      terms.push({ term_id: r.term_id, kind: r.kind || kind });
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

/** Mount a task's agent pane. `onLive(bool)` reports session liveness (for the
 *  quiet idle chip); `onExit(reason)` reacts to closures. Input is allowed — a
 *  live PTY on the user's machine; the resize/input RPCs no-op harmlessly when
 *  no session is live and surface their error to the caller. */
export function mountAgentPane(host, taskId, { onLive, onExit }) {
  const manager = terminalManager();
  const termId = `agent:${taskId}`;
  return mountTerminalPane(host, {
    attach: (opts) => manager.attachAgent(taskId, { ...opts, onLive }),
    input: (data) => manager.input(termId, data),
    resize: (cols, rows) => manager.resize(termId, cols, rows),
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
      },
    };
  });
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
