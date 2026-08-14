// The console: the basement of every work surface.
//
// It sits at the bottom of the view column on the branch and issue surfaces,
// shut by default — a bar that says what it is. Opening it puts the terminals
// of the checkout the work item stands in (a branch's worktree; the primary
// checkout for an issue and for main) at half the view, and the grow control
// lays them over all of it. Each work item remembers the size it was left at.
//
// Terminals are the human's own shells and nothing else: an agent is Build's,
// lives in the agent rail, and is never one of these. The panes ride the ONE
// shared terminal socket (terminal/manager.js), demuxed by term_id, so opening
// the console costs no second connection.

import { App } from "../app.js";
import {
  consoleKey,
  consoleScope,
  consoleTakesKey,
  grownConsoleSize,
  readConsoleSize,
  takeConsoleTerminal,
  toggledConsoleSize,
  writeConsoleSize,
} from "./consoleModel.js";
import { attachConnectionOverlay } from "./surfaceTabs.js";
import { esc } from "./text.js";
import { terminalManager } from "../terminal/manager.js";
import { mountTerminalPane } from "../terminal/pane.js";
import "../styles/shell.css";

/** Which terminal each work item was last looking at. The console is rebuilt
 *  whenever the surface under it is (a tab switch re-renders the view), so the
 *  human's choice is kept here rather than in the DOM that is replaced. */
const chosenTerminal = new Map();

/** Forget what the console remembers. For tests, and for a session teardown. */
export function resetConsoleMemory() {
  chosenTerminal.clear();
}

/** Track a checkout's open user terminals: list on mount, create from the `+`,
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
    /** The tab descriptors for the console head: ordinal-labeled, all closable. */
    tabs: () => terms.map((t) => ({ id: t.term_id, label: labelOf(t.term_id) })),
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
    /** Open one of the user's shells in this checkout's directory. */
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

/** Pure: the console's head — the way in and out, the terminals it holds, and
 *  the two controls that only mean something while it is open. */
export function consoleHeadHtml({ size, tabs = [], selected = null, scoped = true }) {
  const open = size !== "collapsed";
  const toggle = `<button type="button" class="console-bar" id="console-toggle" aria-expanded="${open}"
      title="${open ? "Shut the console" : "Open the console (`)"}">
      <span class="console-caret">${open ? "▼" : "▲"}</span><span class="console-label">Console</span></button>`;
  if (!open) return toggle;
  const cells = tabs
    .map(
      (tab) =>
        `<span class="console-tab${tab.id === selected ? " active" : ""}">` +
        `<button type="button" class="console-tab-name" data-term="${esc(tab.id)}">${esc(tab.label)}</button>` +
        `<span class="tx" data-close="${esc(tab.id)}" title="Close this terminal">×</span></span>`,
    )
    .join("");
  const add = scoped
    ? `<button type="button" class="iconbtn console-new" title="New terminal" aria-label="New terminal">+</button>`
    : "";
  const grow = `<button type="button" class="iconbtn console-grow"
      title="${size === "full" ? "Half the view" : "Over the whole view"}"
      aria-label="${size === "full" ? "Half the view" : "Over the whole view"}">${size === "full" ? "⤡" : "⤢"}</button>`;
  return `${toggle}<div class="console-tabs">${cells}</div><div class="console-controls">${add}${grow}</div>`;
}

/**
 * Mount the console for one work item.
 *
 * `context` is `{ kind: "branch", projectId, branch }` or
 * `{ kind: "issue", projectId, issueId }` — the same address the agent rail
 * takes. Returns `{ dispose(), toggle(), size() }`; disposing tears down the
 * client view only, and never the server PTYs.
 */
export function mountConsole(host, context) {
  if (!host) return { dispose() {}, toggle() {}, size: () => "collapsed" };
  const key = consoleKey(context);
  const manager = terminalManager();
  // A pre-redesign `term-<n>` URL asked for one terminal in particular; that is
  // also a request to open on it. Taken (and spent) whatever kind of work item
  // this is, so a stale mark cannot open some later console on a stranger.
  const wanted = takeConsoleTerminal();
  const wantedHere = context && context.kind === "branch" ? wanted : null;
  let size = wantedHere ? "half" : readConsoleSize(key);
  let scope = null; // the checkout's terminal scope, once resolved
  let terms = null; // the controller, once there is a scope to list
  let selected = chosenTerminal.get(key) || null;
  let loading = false;
  let unresolved = false; // the branch named no directory to stand in
  let pane = null;
  let paneTermId = null; // which terminal the mounted pane is showing
  let connection = null;
  let disposed = false;

  // ---- the terminals ---------------------------------------------------------

  const resolveScope = async () => {
    if (context.kind === "issue") return consoleScope(context, null);
    let row = null;
    try {
      row = await App.call("branch.get", { project_id: context.projectId, branch: context.branch });
    } catch {
      // A branch that stopped resolving (finished, renamed) has no directory to
      // open a shell in; the console says so rather than showing a dead tab.
      return null;
    }
    return consoleScope(context, row);
  };

  /// List the checkout's terminals, once, the first time the console opens.
  /// Nothing is ever created here: opening the console must not spawn a shell
  /// on the user's machine, least of all on a size the last visit remembered.
  const ensureTerminals = async () => {
    if (terms || loading) return;
    loading = true;
    paint();
    scope = await resolveScope();
    if (disposed) return;
    if (!scope) {
      loading = false;
      unresolved = true;
      paint();
      return;
    }
    terms = terminalTabsController(scope);
    await terms.load();
    loading = false;
    if (disposed) return;
    const ids = terms.ids();
    selected = (wantedHere && ids.includes(wantedHere) ? wantedHere : null) || (ids.includes(selected) ? selected : ids[0]) || null;
    remember();
    paint();
  };

  const remember = () => {
    if (selected) chosenTerminal.set(key, selected);
    else chosenTerminal.delete(key);
  };

  const newTerminal = async () => {
    if (!terms) return;
    try {
      selected = await terms.create();
      remember();
    } catch (error) {
      failInBody(`cannot open a terminal: ${(error && error.message) || "error"}`);
      return;
    }
    paint();
  };

  const closeTerminal = async (termId) => {
    if (!terms) return;
    try {
      await terms.close(termId);
    } catch {
      /* raced with the reaper — drop the tab regardless */
    }
    afterTerminalGone(termId);
  };

  /** A terminal that is no longer there: its tab goes, and the console falls
   *  back to whatever is left. */
  const afterTerminalGone = (termId) => {
    if (terms) terms.drop(termId);
    if (selected === termId) {
      selected = (terms && terms.ids()[0]) || null;
      remember();
    }
    if (paneTermId === termId) disposePane();
    paint();
  };

  // ---- painting --------------------------------------------------------------

  /// The head is rewritten on every state change — it is a few buttons and its
  /// whole job is to be current. The BODY is not: it is where a live PTY hangs,
  /// so it is rebuilt only when the terminal in it changes.
  const paint = () => {
    if (disposed) return;
    if (!host.querySelector(".console")) {
      host.innerHTML = `<div class="console"><div class="console-head"></div><div class="console-body"></div></div>`;
    }
    host.dataset.size = size;
    const head = host.querySelector(".console-head");
    head.innerHTML = consoleHeadHtml({
      size,
      tabs: terms ? terms.tabs() : [],
      selected,
      scoped: !!terms,
    });
    wireHead(head);
    paintBody();
  };

  const wireHead = (head) => {
    const toggle = head.querySelector("#console-toggle");
    if (toggle) toggle.onclick = () => setSize(toggledConsoleSize(size));
    const grow = head.querySelector(".console-grow");
    if (grow) grow.onclick = () => setSize(grownConsoleSize(size));
    const add = head.querySelector(".console-new");
    if (add) add.onclick = () => newTerminal();
    head.querySelectorAll("[data-term]").forEach((cell) => {
      cell.onclick = () => {
        if (cell.dataset.term === selected) return;
        selected = cell.dataset.term;
        remember();
        paint();
      };
    });
    head.querySelectorAll("[data-close]").forEach((cell) => {
      cell.onclick = () => closeTerminal(cell.dataset.close);
    });
  };

  const body = () => host.querySelector(".console-body");

  const paintBody = () => {
    const region = body();
    if (!region) return;
    if (size === "collapsed") {
      // A shut console holds no screen: the PTY keeps running on the machine,
      // the client stops rendering and streaming it.
      disposePane();
      region.innerHTML = "";
      return;
    }
    // Only once the checkout has answered: a terminal remembered from an
    // earlier visit is not known to still exist until the list says so.
    if (terms && selected) {
      if (paneTermId !== selected) mountPane(region, selected);
      return;
    }
    disposePane();
    region.innerHTML = `<div class="console-empty">${emptyHtml()}</div>`;
    const start = region.querySelector(".console-start");
    if (start) start.onclick = () => newTerminal();
  };

  const emptyHtml = () => {
    if (loading) return `<span class="dim">Opening…</span>`;
    if (unresolved) return `<span class="dim">There is no checkout here to open a terminal in.</span>`;
    return `<span class="dim">No terminals are open in this checkout.</span>
      <button type="button" class="btn console-start">Open a terminal</button>`;
  };

  const failInBody = (message) => {
    const region = body();
    if (region) region.innerHTML = `<div class="console-empty"><span class="dim">${esc(message)}</span></div>`;
  };

  const mountPane = (region, termId) => {
    disposePane();
    paneTermId = termId;
    region.innerHTML = `<div class="termpane console-pane"></div>`;
    const paneHost = region.querySelector(".console-pane");
    mountUserTerminalPane(paneHost, termId, { onExit: () => afterTerminalGone(termId) }).then(
      (mounted) => {
        if (disposed || paneTermId !== termId) {
          mounted.dispose();
          return;
        }
        pane = mounted;
        connection = attachConnectionOverlay(paneHost);
      },
      (error) => {
        if (disposed || paneTermId !== termId) return;
        // "unknown term_id" = the terminal is gone (exited, closed elsewhere,
        // reaped while the console was shut): drop the tab rather than leave a
        // blank pane that fails identically on every click.
        if (/unknown term_id/.test((error && error.message) || "")) afterTerminalGone(termId);
        else paneHost.innerHTML = `<div class="empty">terminal unavailable: ${esc((error && error.message) || "error")}</div>`;
      },
    );
  };

  const disposePane = () => {
    if (connection) connection.dispose();
    if (pane) pane.dispose();
    // Deregister the screen — the server PTY keeps running.
    if (paneTermId) manager.detach(paneTermId);
    connection = null;
    pane = null;
    paneTermId = null;
  };

  // ---- size ------------------------------------------------------------------

  const setSize = (next) => {
    if (next === size) return;
    size = next;
    writeConsoleSize(key, size);
    paint();
    if (size !== "collapsed") ensureTerminals();
  };

  // ---- the keyboard ----------------------------------------------------------

  // The basement's door. A backtick anywhere that is not a field, a terminal or
  // a composer puts the console at half, and the same key puts it away.
  const onKeydown = (event) => {
    if (event.key !== "`" || event.metaKey || event.ctrlKey || event.altKey) return;
    if (!consoleTakesKey(event.target)) return;
    event.preventDefault();
    setSize(toggledConsoleSize(size));
  };
  document.addEventListener("keydown", onKeydown);

  paint();
  if (size !== "collapsed") ensureTerminals();

  return {
    size: () => size,
    toggle: () => setSize(toggledConsoleSize(size)),
    dispose() {
      disposed = true;
      document.removeEventListener("keydown", onKeydown);
      disposePane();
      host.innerHTML = "";
      delete host.dataset.size;
    },
  };
}
