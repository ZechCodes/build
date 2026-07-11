// Shared terminal/agent/files tab machinery for the three worktree-backed
// surfaces (task, external worktree, primary "main" checkout). The tab-row
// rendering is tabshell.js; this owns the DYNAMIC tab set (user terminals) and
// the mounting of a terminal/agent/files body — identical behavior everywhere
// (§7.2). Panes ride the ONE shared terminal socket (manager.js), demuxed by
// term_id; fs.* ride the app RPC session.

import { terminalManager } from "../terminal/manager.js";
import { mountTerminalPane } from "../terminal/pane.js";
import { renderFilesTab } from "../views/files.js";
import { esc } from "./text.js";

/** Track a surface's open user terminals: list on mount, create on `+`, close on
 *  `×`. Ordinal labels ("1", "2", …) come from position in creation/list order. */
export function terminalTabsController(scope) {
  const manager = terminalManager();
  let terms = []; // [{ term_id }] in list/creation order
  return {
    ids: () => terms.map((t) => t.term_id),
    /** The tab descriptors for the shell: closable, ordinal-labeled. */
    tabs: () => terms.map((t, i) => ({ id: t.term_id, label: String(i + 1), closable: true })),
    label: (termId) => {
      const i = terms.findIndex((t) => t.term_id === termId);
      return i < 0 ? "" : String(i + 1);
    },
    has: (termId) => terms.some((t) => t.term_id === termId),
    async load() {
      try {
        terms = (await manager.listTerminals(scope)).map((t) => ({ term_id: t.term_id }));
      } catch {
        terms = []; // an unknown/unresolvable scope means "no terminals"
      }
      return terms;
    },
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
  let disposed = false;
  mountUserTerminalPane(paneHost, tabId, { onExit }).then(
    (p) => (disposed ? p.dispose() : (pane = p)),
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
      if (pane) pane.dispose();
      manager.detach(tabId);
    },
  };
}
