// The console: the basement of every work surface.
//
// It sits at the bottom of the view column on the branch and issue surfaces,
// shut by default — a label, and beside it a scrolling strip of one tab per
// terminal open in the checkout the work item stands in (a branch's worktree;
// the primary checkout for an issue and for main). The strip is the way in:
// pressing a tab puts that terminal's screen at half the view, the + at the end
// of the strip opens another shell, and the grow control lays the panel over
// the whole view. Each work item remembers the size it was left at.
//
// The panel holds a screen or a line saying why there is none; it is never
// shown empty. So a console with no terminals is a bar and a +, the label does
// nothing, and closing the last terminal shuts the panel without spending the
// size it was open at.
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
import { RECONNECTING_MESSAGE, attachConnectionOverlay, whenTerminalReconnects } from "./surfaceTabs.js";
import { el } from "../dom.js";
import { esc } from "./text.js";
import { hide, reveal } from "./motion.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { SMALLEST_THREAD_PAGE } from "./thread.js";
import { terminalManager } from "../terminal/manager.js";
import { cacheDeviceId } from "./cacheScope.js";
import { entityIdOf } from "./entityId.js";
import { readCached, writeCached } from "./localCache.js";
import { subscribeFeed } from "./taskFeed.js";
import { isTerminalSocketLost } from "../terminal/session.js";
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
    /** Stand the list up from the saved tab ids — the cached first paint. */
    seed(termIds) {
      terms = termIds.map((term_id) => ({ term_id }));
    },
    /** The tab descriptors for the console head: ordinal-labeled, all closable. */
    tabs: () => terms.map((t) => ({ id: t.term_id, label: labelOf(t.term_id) })),
    label: labelOf,
    has: (termId) => terms.some((t) => t.term_id === termId),
    async load() {
      try {
        terms = (await manager.listTerminals(scope)).map((t) => ({ term_id: t.term_id }));
      } catch (error) {
        // A machine we cannot reach has not answered the question — reading its
        // silence as "no terminals" is how a shut-looking console gets a shell
        // opened next to the ones already running in that checkout.
        if (isTerminalSocketLost(error)) throw error;
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

/** Pure: the way in and out. The label is the whole control — a caret beside it
 *  would say a second time what the panel under it already says. */
export function consoleToggleHtml(size) {
  const open = size !== "collapsed";
  return `<button type="button" class="console-bar" id="console-toggle" aria-expanded="${open}"
      title="${open ? "Shut the console" : "Open the console (`)"}">
      <span class="console-label">Console</span></button>`;
}

/** Pure: one terminal's tab — its ordinal, and the way to close it. */
export function consoleTabHtml(tab, selected) {
  return (
    `<span class="console-tab${tab.id === selected ? " active" : ""}" data-motion>` +
    `<button type="button" class="console-tab-name" data-term="${esc(tab.id)}">${esc(tab.label)}</button>` +
    `<span class="tx" data-close="${esc(tab.id)}" title="Close this terminal">×</span></span>`
  );
}

/** Pure: the + that opens another shell. It rides at the end of the strip and
 *  sticks to its right edge, so a strip scrolled off the end still offers it. */
export function consoleNewTerminalHtml() {
  return `<button type="button" class="iconbtn console-new" data-motion
      title="New terminal" aria-label="New terminal">+</button>`;
}

/** Pure: the control that lays the console over the whole view, and takes it
 *  back to half. It means nothing while the console is shut. */
export function consoleGrowHtml(size) {
  const label = size === "full" ? "Half the view" : "Over the whole view";
  return `<button type="button" class="iconbtn console-grow" data-motion
      title="${label}" aria-label="${label}">${size === "full" ? "⤡" : "⤢"}</button>`;
}

/** Pure: the console's head — the way in and out, the strip of terminals it
 *  holds with the + at the end of it, and the grow control on the far right. */
export function consoleHeadHtml({ size, tabs = [], selected = null, scoped = true }) {
  const cells = tabs.map((tab) => consoleTabHtml(tab, selected)).join("");
  const add = scoped ? consoleNewTerminalHtml() : "";
  const grow = size === "collapsed" ? "" : consoleGrowHtml(size);
  return `${consoleToggleHtml(size)}<div class="console-tabs scrollstrip">${cells}${add}</div><div class="console-controls">${grow}</div>`;
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
  const remembered = readConsoleSize(key);
  let size = wantedHere ? "half" : remembered; // the size asked for, shown only when there is something to show
  let openedSize = remembered === "full" ? "full" : "half"; // the size an open console takes
  let scope = null; // the checkout's terminal scope, once resolved
  let terms = null; // the controller, once there is a scope to list
  let selected = chosenTerminal.get(key) || null;
  let loading = false;
  let unresolved = false; // the branch named no directory to stand in
  let unreachable = false; // the machine did not answer — not the same as empty
  let createFailure = null; // why the last + could not open a shell
  let pane = null;
  let paneTermId = null; // which terminal the mounted pane is showing
  let connection = null;
  let reconnectWatch = null; // the one-shot wait for the socket to come back
  let disposed = false;

  /** Do this again once the terminal socket is back. Only one wait at a time:
   *  the list and the pane are steps of the same mount, so the later one
   *  replaces whatever the earlier one was waiting to redo. */
  const retryWhenReconnected = (retry) => {
    if (reconnectWatch) reconnectWatch.dispose();
    reconnectWatch = whenTerminalReconnects(() => {
      reconnectWatch = null;
      if (!disposed) retry();
    });
  };

  // ---- the terminals ---------------------------------------------------------

  const resolveScope = async () => {
    if (context.kind === "issue") return consoleScope(context, null);
    let row = null;
    try {
      row = await App.call("branch.get", {
        project_id: context.projectId,
        branch: context.branch,
        ...SMALLEST_THREAD_PAGE,
      });
    } catch {
      // A branch that stopped resolving (finished, renamed) has no directory to
      // open a shell in; the console says so rather than showing a dead tab.
      return null;
    }
    return consoleScope(context, row);
  };

  /** The feed's row for this branch, read off the shared snapshot without
   *  subscribing — the replay-to-late-subscribers path, used synchronously. */
  const feedRowNow = () => {
    let row = null;
    const unsubscribe = subscribeFeed((feed) => {
      row =
        (feed.items || []).find(
          (item) => item.kind === "branch" && item.project_id === context.projectId && item.branch === context.branch,
        ) || null;
    });
    unsubscribe();
    return row;
  };

  /** The local cache's address for this checkout's tab list, or null while the
   *  entity is unknown. Cached tabs paint the head without a round trip. */
  const tabsCacheAddress = () => {
    const deviceId = cacheDeviceId();
    const entityId = context.kind === "issue" ? context.issueId : entityIdOf(feedRowNow());
    return deviceId && entityId ? { deviceId, entityId, kind: "tabs" } : null;
  };

  const pickSelected = () => {
    const ids = terms.ids();
    selected = (wantedHere && ids.includes(wantedHere) ? wantedHere : null) || (ids.includes(selected) ? selected : ids[0]) || null;
  };

  /** The live list, folded over whatever the cache painted: the scope is
   *  re-resolved (an adoption may have moved it), the tabs are re-listed, and
   *  the answer is written through for the next visit. */
  const reconcileTerminals = async (address) => {
    const liveScope = await resolveScope();
    if (disposed || !liveScope) return;
    const live = terminalTabsController(liveScope);
    try {
      await live.load();
    } catch {
      // Out of reach: the cached tabs stand, and the machine coming back
      // reconciles them then.
      if (!disposed) retryWhenReconnected(() => reconcileTerminals(address));
      return;
    }
    if (disposed) return;
    scope = liveScope;
    terms = live;
    pickSelected();
    remember();
    paint();
    writeCached(address, { scope, termIds: terms.ids() });
  };

  /// List the checkout's terminals, once, as the console is stood up — the
  /// strip says what is open there whether the panel is shut or not.
  /// Nothing is ever created here: standing the console up must not spawn a
  /// shell on the user's machine, least of all on a size the last visit
  /// remembered.
  ///
  /// The saved tab list paints the head first, without a round trip; the live
  /// list reconciles it the moment it lands.
  const ensureTerminals = async () => {
    if (terms || loading) return;
    loading = true;
    paint();
    const address = tabsCacheAddress();
    const saved = address ? await readCached(address) : undefined;
    if (disposed || terms) return;
    if (saved && saved.value && saved.value.scope && Array.isArray(saved.value.termIds)) {
      scope = saved.value.scope;
      terms = terminalTabsController(scope);
      terms.seed(saved.value.termIds);
      loading = false;
      pickSelected();
      paint();
      reconcileTerminals(address);
      return;
    }
    scope = await resolveScope();
    if (disposed) return;
    if (!scope) {
      loading = false;
      unresolved = true;
      paint();
      return;
    }
    terms = terminalTabsController(scope);
    try {
      await terms.load();
    } catch {
      // The socket was not there. Nothing was listed, so nothing is known yet:
      // say the machine is out of reach and ask again when it is back.
      terms = null;
      loading = false;
      unreachable = true;
      if (disposed) return;
      paint();
      retryWhenReconnected(ensureTerminals);
      return;
    }
    loading = false;
    unreachable = false;
    if (disposed) return;
    pickSelected();
    remember();
    paint();
    if (address) writeCached(address, { scope, termIds: terms.ids() });
  };

  const remember = () => {
    if (selected) chosenTerminal.set(key, selected);
    else chosenTerminal.delete(key);
  };

  /** Keep the saved tab list telling the truth after a create or a close. */
  const persistTabs = () => {
    const address = tabsCacheAddress();
    if (address && scope && terms) writeCached(address, { scope, termIds: terms.ids() });
  };

  /// The + opens a shell AND the panel: a terminal nobody can see is not what
  /// was asked for. A create that failed opens it too — on the reason.
  const newTerminal = async () => {
    if (!terms) return;
    try {
      selected = await terms.create();
      createFailure = null;
      remember();
    } catch (error) {
      createFailure = `cannot open a terminal: ${(error && error.message) || "error"}`;
      openPanel();
      return;
    }
    persistTabs();
    openPanel();
    scrollStripToNewest();
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
    persistTabs();
    if (selected === termId) {
      selected = (terms && terms.ids()[0]) || null;
      remember();
    }
    if (paneTermId === termId) disposePane();
    paint();
  };

  // ---- painting --------------------------------------------------------------

  /// The head is stood up once and then kept: its terminals are a keyed list,
  /// so a tab grows into the strip when it arrives and shrinks out of it when
  /// it goes, and a selection or a size patches the tabs already standing. The
  /// BODY is where a live PTY hangs, so it is rebuilt only when the terminal in
  /// it changes.
  const paint = () => {
    if (disposed) return;
    if (!host.querySelector(".console")) {
      host.innerHTML = `<div class="console"><div class="console-head"></div><div class="console-body"></div></div>`;
    }
    const shown = shownSize();
    host.dataset.size = shown;
    paintHead(host.querySelector(".console-head"), shown);
    paintBody(shown);
  };

  const paintHead = (head, shown) => {
    if (!head.querySelector(".console-tabs")) {
      head.innerHTML = consoleHeadHtml({ size: shown, tabs: [], selected, scoped: false });
      wireHead(head);
    }
    patchElement(head.querySelector("#console-toggle"), el(consoleToggleHtml(shown)));
    paintTabs(head.querySelector(".console-tabs"));
    paintGrowControl(head.querySelector(".console-controls"), shown);
  };

  /// The toggle is the one control that is there whatever the console is doing,
  /// so it is the one wired to the element rather than to a paint.
  const wireHead = (head) => {
    head.querySelector("#console-toggle").onclick = () => toggleConsole();
  };

  const paintTabs = (strip) => {
    patchList(strip, terms ? terms.tabs() : [], {
      keyOf: (tab) => tab.id,
      render: (tab) => consoleTabHtml(tab, selected),
      wire: (tab) => wireTab(tab),
      onEnter: (tab) => {
        tab.hidden = true;
        return reveal(tab, { axis: "width" });
      },
      onExit: (tab) => hide(tab, { axis: "width" }),
    });
    paintNewTerminalControl(strip);
  };

  /// A tab answers for whichever terminal it is keyed to at the time it is
  /// clicked — the element outlives every list it was painted from.
  const wireTab = (tab) => {
    tab.querySelector(".console-tab-name").onclick = () => selectTerminal(tab.dataset.key);
    tab.querySelector(".tx").onclick = () => closeTerminal(tab.dataset.key);
  };

  /// The + is the strip's last cell, and only where there is a checkout to open
  /// a shell in.
  const paintNewTerminalControl = (strip) => {
    const standing = strip.querySelector(".console-new");
    if (!terms) {
      if (standing) hide(standing, { axis: "width" });
      return;
    }
    if (!standing) {
      const add = el(consoleNewTerminalHtml());
      add.hidden = true;
      add.onclick = () => newTerminal();
      strip.appendChild(add);
      reveal(add, { axis: "width" });
      return;
    }
    if (standing.nextSibling) strip.appendChild(standing);
    reveal(standing, { axis: "width" });
  };

  const paintGrowControl = (controls, shown) => {
    const standing = controls.querySelector(".console-grow");
    if (shown === "collapsed") {
      if (standing) hide(standing, { axis: "width" });
      return;
    }
    if (!standing) {
      const grow = el(consoleGrowHtml(shown));
      grow.hidden = true;
      grow.onclick = () => growPanel();
      controls.appendChild(grow);
      reveal(grow, { axis: "width" });
      return;
    }
    patchElement(standing, el(consoleGrowHtml(shown)));
    reveal(standing, { axis: "width" });
  };

  /// A tab is a way into the console as well as a choice of terminal: pressing
  /// one on a shut console opens it on that terminal.
  const selectTerminal = (termId) => {
    if (termId !== selected) {
      selected = termId;
      remember();
    }
    openPanel();
  };

  /// The newest tab and the + are what a create is about, so the strip is put
  /// where they are rather than where it was left.
  const scrollStripToNewest = () => {
    const strip = host.querySelector(".console-tabs");
    if (strip) strip.scrollLeft = strip.scrollWidth;
  };

  const body = () => host.querySelector(".console-body");

  const paintBody = (shown) => {
    const region = body();
    if (!region) return;
    if (shown === "collapsed") {
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
    region.innerHTML = `<div class="console-empty"><span class="dim">${esc(bodyMessage())}</span></div>`;
  };

  /// What the body has to say when there is no screen in it, and "" when it has
  /// nothing to say — which is the state the console is never shown in.
  const bodyMessage = () => {
    if (createFailure) return createFailure;
    if (loading) return "Opening…";
    // The machine being out of reach is not the same as it holding no
    // terminals, and neither is a branch that names no directory at all.
    if (unreachable) return RECONNECTING_MESSAGE;
    if (unresolved) return "There is no checkout here to open a terminal in.";
    return "";
  };

  /** Whether the panel would hold anything: a terminal's screen, or a line
   *  saying why there is none. */
  const hasSomethingToShow = () => !!((terms && selected) || bodyMessage());

  /// A panel with nothing in it says nothing, so it is not shown: the console is
  /// drawn at the size it was asked for while it holds a screen or a message,
  /// and shut whenever it holds neither. The size it was asked for is kept
  /// through that, so the terminal that opens next opens the console with it.
  const shownSize = () => (hasSomethingToShow() ? size : "collapsed");

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
        // The socket died under the attach: the PTY is fine, this browser just
        // cannot see it. Say so where the screen would be, and mount again when
        // the socket is back — a terminal called "unavailable" here would stay
        // that way until the human clicked something.
        if (isTerminalSocketLost(error)) {
          paneHost.innerHTML = `<div class="empty">${esc(RECONNECTING_MESSAGE)}</div>`;
          retryWhenReconnected(() => {
            if (paneTermId !== termId) return; // the console is showing something else now
            paneTermId = null; // …otherwise mount this terminal afresh
            paintBody(shownSize());
          });
          return;
        }
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
    size = next;
    if (next !== "collapsed") openedSize = next;
    writeConsoleSize(key, next);
    paint();
  };

  /// Show the panel on whatever the console is holding. A console already at a
  /// size keeps it — the create or the tab press was about the terminal.
  const openPanel = () => {
    if (size === "collapsed") setSize(openedSize);
    else paint();
  };

  const growPanel = () => setSize(grownConsoleSize(shownSize()));

  /// The label, and the backtick. With nothing to show it is inert: there is no
  /// empty panel to open, and the size the console is remembered at is left as
  /// it is rather than being written over by a press that did nothing.
  const toggleConsole = () => {
    if (!hasSomethingToShow()) return;
    setSize(toggledConsoleSize(shownSize(), openedSize));
  };

  // ---- the keyboard ----------------------------------------------------------

  // The basement's door. A backtick anywhere that is not a field, a terminal or
  // a composer puts the console at half, and the same key puts it away.
  const onKeydown = (event) => {
    if (event.key !== "`" || event.metaKey || event.ctrlKey || event.altKey) return;
    if (!consoleTakesKey(event.target)) return;
    event.preventDefault();
    toggleConsole();
  };
  document.addEventListener("keydown", onKeydown);

  paint();
  // The strip says what is open in this checkout whether the panel is shut or
  // not, so the listing is part of standing the console up. Listing only: a
  // console has never created a shell it was not asked for.
  ensureTerminals();

  return {
    size: () => shownSize(),
    toggle: () => toggleConsole(),
    dispose() {
      disposed = true;
      document.removeEventListener("keydown", onKeydown);
      if (reconnectWatch) reconnectWatch.dispose();
      reconnectWatch = null;
      disposePane();
      host.innerHTML = "";
      delete host.dataset.size;
    },
  };
}
