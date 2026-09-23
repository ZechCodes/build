// The console: the basement of every work surface.
//
// It sits at the bottom of the view column on the branch and issue surfaces.
//
// Terminals are the human's own shells and nothing else: an agent is Build's,
// lives in the agent rail, and is never one of these. The panes ride the ONE
// shared terminal socket (terminal/manager.js), demuxed by term_id, so opening
// the console costs no second connection.
//
// Everything the head draws comes off the cache: which checkout this console
// stands in is the routed row's, and the tab strip is the `terminals` record
// the sync layer writes and every `terminals` push moves. The console asks the
// machine nothing to paint — `term.create` and `term.close` are the only two
// calls left, and both are mutations the reader pressed for.

import {
  DEFAULT_OPEN_SIZE,
  consoleFeedRoute,
  consoleKey,
  consoleScope,
  consoleTakesKey,
  consoleSize,
  grownConsoleSize,
  takeConsoleTerminal,
  toggledConsoleSize,
} from "./consoleModel.js";
import { RECONNECTING_MESSAGE, attachConnectionOverlay, whenTerminalReconnects } from "./surfaceTabs.js";
import { el } from "../dom.js";
import { esc } from "./text.js";
import { hide, motionHooks, motionSettled, reveal } from "./motion.js";
import { notifyError } from "./notify.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { terminalManager } from "../terminal/manager.js";
import { branchRowIn } from "./feedRows.js";
import { ROW_RECORD_KIND, cachedFeedView } from "./cachedRows.js";
import { routedEntityId } from "./inbox.js";
import { directoryCacheId } from "./directoryScope.js";
import { mergeCached, readCached, subscribeCache, writeCached } from "./localCache.js";
import { isTerminalSocketLost } from "../terminal/session.js";
import { mountTerminalPane } from "../terminal/pane.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import "../styles/shell.css";

const TAB_MOTION = motionHooks({ axis: "width" });

/** The record the tab strip is: what the sync layer writes from `term.list`
 *  and every `terminals` push carries. */
const TERMINALS_RECORD_KIND = "terminals";

/** The console's own record. It keeps one thing — which tab was open — and
 *  nothing the bridge is the author of. */
const CONSOLE_RECORD_KIND = "console";

/** Which terminal each work item was last looking at. The console is rebuilt
 *  whenever the surface under it is (a tab switch re-renders the view), so the
 *  human's choice is kept here rather than in the DOM that is replaced. */
const chosenTerminal = new Map();

/** Forget what the console remembers. For tests, and for a session teardown. */
export function resetConsoleMemory() {
  chosenTerminal.clear();
}

/** Track a checkout's open user terminals, seeded only from the `terminals`
 *  record. Labels are ordinals over the order the record lists them in. Pulls,
 *  mutations and pushes all write that record; its announcement is the only
 *  way this controller is moved. */
export function terminalTabsController() {
  let terms = []; // the record's tabs, in the order it lists them
  const labelOf = (termId) => {
    const index = terms.findIndex((t) => t.term_id === termId);
    return index < 0 ? "" : `Terminal ${index + 1}`;
  };
  return {
    ids: () => terms.map((t) => t.term_id),
    /** Stand the strip up from the record. */
    seed(tabs) {
      terms = (tabs || []).filter((tab) => tab && tab.term_id);
    },
    /** The tab descriptors for the console head: ordinal-labeled, all closable. */
    tabs: () => terms.map((t) => ({ id: t.term_id, label: labelOf(t.term_id) })),
    label: labelOf,
    has: (termId) => terms.some((t) => t.term_id === termId),
  };
}

/** Mount a user-terminal pane bound to `termId` on the shared socket. */
export function mountUserTerminalPane(host, termId, { scope = {}, onExit }) {
  const manager = terminalManager();
  return mountTerminalPane(host, {
    attach: (opts) => manager.attachTerminal(termId, scope, opts),
    input: (data) => manager.input(termId, data),
    resize: (cols, rows) => manager.resize(termId, cols, rows),
    onExit,
  });
}

export function consoleToggleHtml(size) {
  const open = size !== "collapsed";
  return `<button type="button" class="console-bar" id="console-toggle" aria-expanded="${open}"
      title="${open ? "Shut the console" : "Open the console (`)"}">
      <span class="console-label">Console</span></button>`;
}

export function consoleTabHtml(tab, selected) {
  return (
    `<span class="console-tab${tab.id === selected ? " active" : ""}" data-motion>` +
    `<button type="button" class="console-tab-name" data-term="${esc(tab.id)}">${esc(tab.label)}</button>` +
    `<span class="tx" data-close="${esc(tab.id)}" title="Close this terminal">×</span></span>`
  );
}

export function consoleNewTerminalHtml() {
  return `<button type="button" class="iconbtn console-new" data-motion
      title="New terminal" aria-label="New terminal">+</button>`;
}

export function consoleGrowHtml(size) {
  const label = size === "full" ? "Half the view" : "Over the whole view";
  return `<button type="button" class="iconbtn console-grow" data-motion
      title="${label}" aria-label="${label}">${size === "full" ? "⤡" : "⤢"}</button>`;
}

export function consoleHeadHtml(size) {
  return `${consoleToggleHtml(size)}<div class="console-tabs scrollstrip"></div><div class="console-controls"></div>`;
}

/**
 * Mount the console for one work item.
 *
 * `context` is `{ kind: "branch", projectId, branch }` or
 * `{ kind: "issue", projectId, issueId }` — the same address the agent rail
 * takes — plus the machine that checkout is on: its `deviceId`, its `call` and
 * its `cacheScope`, all handed down by the view the link opened. Returns
 * `{ dispose(), toggle(), size() }`; disposing tears down the client view only,
 * and never the server PTYs.
 */
export function mountConsole(host, context) {
  if (!host) return { dispose() {}, toggle() {}, size: () => "collapsed" };
  const cacheScope = context.cacheScope;
  const key = consoleKey(context);
  const manager = terminalManager();
  // A pre-redesign `term-<n>` URL asked for one terminal in particular; that is
  // also a request to open on it. Taken (and spent) whatever kind of work item
  // this is, so a stale mark cannot open some later console on a stranger.
  const wanted = takeConsoleTerminal();
  const wantedHere = context && context.kind === "branch" ? wanted : null;
  let requestedSize = "collapsed";
  let reopenSize = DEFAULT_OPEN_SIZE;
  let sizeKnown = false;
  let wantedApplied = false;
  let sizeRecord;
  let entityId = null; // which entity's records this console is standing in
  let scope = null; // the checkout's terminal scope, once the row names one
  let terms = null; // the controller, once there is a scope to stand it on
  let selected = chosenTerminal.get(key) || null;
  let selectionRead = Boolean(selected); // whether the saved pick has been consulted
  let writtenSelection; // the pick the console record was last told about
  let unresolved = false; // the route named no checkout to stand in
  let tabsKnown = false; // whether anything has said what this checkout is holding
  let listing = false; // the one list of a checkout no record has answered for
  let unreachable = false; // that list did not get through — not "no terminals"
  let terminalsAt; // write time of the terminal record the strip last read
  let scrollToTerminal = null; // a created tab to reveal after its record lands
  let pane = null;
  let paneTermId = null; // which terminal the mounted pane is showing
  let connection = null;
  let reconnectWatch = null; // the one-shot wait for the socket to come back
  let disposed = false;
  let unwatchCache = null;
  let watchedEntity; // the entity the cache watch is pointed at
  let place = null; // where this console stands, re-read only when a row moves

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

  // A routed entity is where the sync layer and pushes file this checkout. A
  // finished branch may no longer resolve to one while its cached row still
  // names the run/worktree whose shells survive; file that fallback list under
  // the checkout id, just as its directory-local cache is.
  const recordEntityId = () => entityId || directoryCacheId(scope);
  const cacheAddress = (kind) => {
    const cacheEntity = recordEntityId();
    return cacheEntity ? cacheScope?.address({ entityId: cacheEntity, kind }) || null : null;
  };

  const cachedValue = async (kind) => {
    const address = cacheAddress(kind);
    return address ? (await readCached(address))?.value : undefined;
  };

  const writeThrough = (kind, value) => {
    const address = cacheAddress(kind);
    if (address) writeCached(address, value); // fire and forget
  };

  /** Where this console stands, off the device's own rows: the entity its
   *  records are filed under, and the checkout its shells run in. An issue is
   *  its own entity and its agent runs in the project's checkout, so neither
   *  answer needs a row. */
  const readPlace = async () => {
    if (context.kind === "issue") return { entityId: context.issueId || null, scope: consoleScope(context, null) };
    const view = await cachedFeedView(context.deviceId);
    const row = context.kind === "branch" ? branchRowIn(view, context.projectId, context.branch) : null;
    return { entityId: routedEntityId(consoleFeedRoute(context), view), scope: consoleScope(context, row) };
  };

  /** Whether a record that moved is one the place is read off. Resolving the
   *  place walks every row the device holds, and one sync pass writes a dozen
   *  records under this entity — a status, a log, a thread, this console's own
   *  two — none of which can move where this console stands. Only the rows and
   *  the two lists a route is named by can, and an announcement that does not
   *  say what moved is taken as one that could. */
  const movesPlace = (changed) =>
    !changed?.kind || changed.kind === ROW_RECORD_KIND || changed.kind === "projects" || changed.kind === "workspaces";

  const pickSelected = () => {
    const ids = terms.ids();
    selected = (wantedHere && ids.includes(wantedHere) ? wantedHere : null) || (ids.includes(selected) ? selected : ids[0]) || null;
  };

  /** The tab this console was last left on, read once. The in-session memory
   *  leads: it is this tab's own most recent word, and it answers without a
   *  round trip to the disk. */
  const readSavedSelection = async () => {
    if (selectionRead) return;
    selectionRead = true;
    const saved = (await cachedValue(CONSOLE_RECORD_KIND))?.selected;
    if (saved && !selected) selected = saved;
  };

  /** Point the cache watch at the narrowest thing that can answer: this
   *  console's own entity once a row names it, and the device as a whole while
   *  no row does — a branch adopted under an open console gains its row then. */
  const watchCache = () => {
    const at = entityId || "";
    if (watchedEntity === at) return;
    const address = cacheScope?.address(entityId ? { entityId } : {});
    if (!address) return;
    watchedEntity = at;
    unwatchCache?.();
    unwatchCache = subscribeCache(address, (changed) => void takeUpCache(changed));
  };

  const standOnScope = (liveScope) => {
    if (terms && JSON.stringify(scope) === JSON.stringify(liveScope)) return;
    scope = liveScope;
    terms = terminalTabsController();
    // Another checkout: nothing said yet about what THIS one is holding.
    tabsKnown = false;
    unreachable = false;
    terminalsAt = undefined;
  };

  /** Seed the strip from its record. A checkout no routed entity names falls
   *  back to its directory cache id above, so even a finished branch's pull is
   *  committed and announced before its surviving shells can paint. */
  const seedFromRecord = async () => {
    const address = cacheAddress(TERMINALS_RECORD_KIND);
    if (!address) return;
    const record = await readCached(address);
    if (disposed) return;
    terminalsAt = record?.at;
    const strip = record?.value;
    if (strip) tabsKnown = true;
    terms.seed(strip?.tabs || []);
  };

  /** Read what the cache says and paint it: the checkout, the tab strip, and
   *  the tab this console was left on. */
  const readCacheOnce = async (placeMoved) => {
    if (!place || placeMoved) place = await readPlace();
    if (disposed) return;
    entityId = place.entityId;
    unresolved = !place.scope;
    watchCache();
    if (!place.scope) {
      terms = null;
      paint();
      return;
    }
    standOnScope(place.scope);
    await readSavedSelection();
    if (disposed) return;
    await seedFromRecord();
    if (disposed) return;
    pickSelected();
    remember();
    paint();
    if (scrollToTerminal && terms.has(scrollToTerminal)) {
      scrollToTerminal = null;
      scrollStripToNewest();
    }
    if (!tabsKnown) void listOnce();
  };

  /** The one list. A checkout no record has ever answered for is not a checkout
   *  with no shells: reading that silence as "no terminals" is how a
   *  shut-looking console gets a shell opened next to the ones already running
   *  in it. So before any sync pass or `terminals` push has reached this entity
   *  — a first visit, a sign-out wipe — the console asks once and writes the
   *  answer where the strip reads it. From then on the record is the strip. */
  const listOnce = async () => {
    // Asked once, and not again until the socket says it is back: a console
    // that re-asked on every cache announcement would stack retries behind a
    // machine that is not answering.
    if (listing || tabsKnown || unreachable || !terms) return;
    const asked = terms;
    const address = cacheAddress(TERMINALS_RECORD_KIND);
    if (!address) return;
    const beforeAt = terminalsAt;
    listing = true;
    let tabs;
    try {
      tabs = (await manager.listTerminals(scope)).filter((tab) => tab && tab.term_id);
    } catch (error) {
      if (isTerminalSocketLost(error)) {
        listLost(asked);
        return;
      }
      // The machine answered but cannot list this scope: it holds no shells.
      tabs = [];
    }
    await listAnswered(asked, address, beforeAt, tabs);
  };

  /** Still unknown. Say the machine is out of reach, keep the `+` back —
   *  creating a shell needs the socket anyway — and ask again when it is back,
   *  on the socket's own reconnect rather than a timer of this console's. */
  const listLost = (asked) => {
    listing = false;
    if (disposed || terms !== asked) return;
    unreachable = true;
    paint();
    retryWhenReconnected(() => {
      unreachable = false;
      void listOnce();
    });
  };

  /** The checkout answered. The record is what the strip reads, so the answer
   *  goes there, and every later word about these shells is a push. */
  const listAnswered = async (asked, address, beforeAt, tabs) => {
    if (disposed || terms !== asked) {
      listing = false;
      return;
    }
    const current = await readCached(address);
    if (disposed || terms !== asked || current?.at !== beforeAt) {
      listing = false;
      return;
    }
    unreachable = false;
    // No payload reaches the controller. The committed write announces its
    // address, and takeUpCache reads it back before the strip can move.
    await writeCached(address, { tabs });
    listing = false;
  };

  // One read at a time, and one more where the cache moved while it ran: a
  // sync pass writes a row, a terminals record and a console record in a
  // burst, and resolving the route walks every row the device holds.
  let reading = null;
  let readAgain = false;
  let placeMoved = false; // a row or a list moved since the place was last read
  const takeUpCache = (changed) => {
    if (movesPlace(changed)) placeMoved = true;
    if (reading) {
      readAgain = true;
      return reading;
    }
    reading = (async () => {
      do {
        readAgain = false;
        const moved = placeMoved;
        placeMoved = false;
        await readCacheOnce(moved);
      } while (readAgain && !disposed);
    })().finally(() => {
      reading = null;
    });
    return reading;
  };

  const changeTabs = (change) => {
    const address = cacheAddress(TERMINALS_RECORD_KIND);
    if (!address) return Promise.resolve();
    return mergeCached(address, (current) => {
      const tabs = (current?.tabs || []).filter((tab) => tab && tab.term_id);
      const next = change(tabs);
      return next === tabs ? null : { tabs: next };
    });
  };

  /** Which terminal this console is on: in memory for the rest of the session,
   *  and in the `console` record for the next visit. Written only where the
   *  pick actually moved — every write announces, and a write per read would
   *  have the watch below re-reading itself for ever. */
  const remember = () => {
    if (selected) chosenTerminal.set(key, selected);
    else chosenTerminal.delete(key);
    if (selected === writtenSelection) return;
    writtenSelection = selected;
    writeThrough(CONSOLE_RECORD_KIND, { selected });
  };

  const newTerminal = async () => {
    if (!terms) return;
    const asked = terms;
    let created;
    try {
      created = await manager.createTerminal(scope, 80, 24);
    } catch (error) {
      notifyError("Could not open a terminal", (error && error.message) || "error");
      return;
    }
    if (disposed || terms !== asked || !created?.term_id) return;
    selected = created.term_id;
    scrollToTerminal = created.term_id;
    if (requestedSize === "collapsed") {
      await setSize(reopenSize);
    }
    await changeTabs((tabs) =>
      tabs.some((tab) => tab.term_id === created.term_id) ? [...tabs] : [...tabs, { term_id: created.term_id }],
    );
  };

  const closeTerminal = async (termId) => {
    try {
      await manager.closeTerminal(termId);
    } catch {
      /* raced with the reaper — its durable list still drops the tab */
    }
    await afterTerminalGone(termId);
  };

  /** A terminal that is no longer there: its tab goes, and the console falls
   *  back to whatever is left. */
  const afterTerminalGone = (termId) =>
    changeTabs((tabs) => {
      const next = tabs.filter((tab) => tab.term_id !== termId);
      return next.length === tabs.length ? tabs : next;
    });

  // ---- painting --------------------------------------------------------------

  const paint = () => {
    if (disposed || !sizeKnown) return;
    if (!host.querySelector(".console")) {
      host.innerHTML = `<div class="console"><div class="console-head"></div><div class="console-body" hidden></div></div>`;
    }
    const drawn = drawnSize();
    host.dataset.size = drawn;
    paintHead(host.querySelector(".console-head"), drawn);
    paintBody(drawn);
  };

  const paintHead = (head, drawn) => {
    if (!head.querySelector(".console-tabs")) {
      head.innerHTML = consoleHeadHtml(drawn);
      wireHead(head);
    }
    patchElement(head.querySelector("#console-toggle"), el(consoleToggleHtml(drawn)));
    paintTabs(head.querySelector(".console-tabs"));
    paintGrowControl(head.querySelector(".console-controls"), drawn);
  };

  const wireHead = (head) => {
    head.querySelector("#console-toggle").onclick = () => toggleConsole();
  };

  const paintTabs = (strip) => {
    patchList(strip, terms ? terms.tabs() : [], {
      keyOf: (tab) => tab.id,
      render: (tab) => consoleTabHtml(tab, selected),
      wire: (tab) => wireTab(tab),
      ...TAB_MOTION,
    });
    paintNewTerminalControl(strip);
  };

  const wireTab = (tab) => {
    tab.querySelector(".console-tab-name").onclick = () => selectTerminal(tab.dataset.key);
    tab.querySelector(".tx").onclick = () => closeTerminal(tab.dataset.key);
  };

  const paintNewTerminalControl = (strip) => {
    const standing = strip.querySelector(".console-new");
    // Offered only over a checkout something has answered for: a `+` over an
    // unanswered one opens a second shell beside whatever is already running.
    if (!terms || !tabsKnown) {
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

  const paintGrowControl = (controls, drawn) => {
    const standing = controls.querySelector(".console-grow");
    if (drawn === "collapsed") {
      if (standing) hide(standing, { axis: "width" });
      return;
    }
    if (!standing) {
      const grow = el(consoleGrowHtml(drawn));
      grow.hidden = true;
      grow.onclick = () => growPanel();
      controls.appendChild(grow);
      reveal(grow, { axis: "width" });
      return;
    }
    patchElement(standing, el(consoleGrowHtml(drawn)));
    reveal(standing, { axis: "width" });
  };

  const selectTerminal = (termId) => {
    if (termId !== selected) {
      selected = termId;
      remember();
    }
    openPanel();
  };

  const scrollStripToNewest = () =>
    motionSettled().then(() => {
      const strip = host.querySelector(".console-tabs");
      if (strip) strip.scrollLeft = strip.scrollWidth;
    });

  const body = () => host.querySelector(".console-body");

  const paintBody = (drawn) => {
    const region = body();
    if (!region) return;
    if (drawn === "collapsed") {
      hide(region, { axis: "height" }).then(() => {
        if (disposed || drawnSize() !== "collapsed") return;
        disposePane();
        region.innerHTML = "";
      });
      return;
    }
    reveal(region, { axis: "height" });
    // Only once the checkout has answered: a terminal remembered from an
    // earlier visit is not known to still exist until the list says so.
    if (terms && selected) {
      if (paneTermId !== selected) mountPane(region, selected);
      return;
    }
    disposePane();
    region.innerHTML = `<div class="console-empty"><span class="dim">${esc(bodyMessage())}</span></div>`;
  };

  const bodyMessage = () => {
    if (unresolved) return "There is no checkout here to open a terminal in.";
    return unreachable ? RECONNECTING_MESSAGE : "";
  };

  const hasSomethingToShow = () => !!((terms && selected) || bodyMessage());

  const drawnSize = () => (hasSomethingToShow() ? requestedSize : "collapsed");

  const mountPane = (region, termId) => {
    disposePane();
    paneTermId = termId;
    region.innerHTML = `<div class="termpane console-pane"></div>`;
    const paneHost = region.querySelector(".console-pane");
    mountUserTerminalPane(paneHost, termId, { scope, onExit: () => void afterTerminalGone(termId) }).then(
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
            paintBody(drawnSize());
          });
          return;
        }
        // "unknown term_id" = the terminal is gone (exited, closed elsewhere,
        // reaped while the console was shut): drop the tab rather than leave a
        // blank pane that fails identically on every click.
        if (/unknown term_id/.test((error && error.message) || "")) void afterTerminalGone(termId);
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
    return sizeRecord.write({ size: next, reopenSize: next === "collapsed" ? reopenSize : next });
  };

  const openPanel = () => {
    if (requestedSize === "collapsed") setSize(reopenSize);
    else paint();
  };

  const growPanel = () => setSize(grownConsoleSize(drawnSize()));

  const toggleConsole = () => {
    if (!hasSomethingToShow()) return;
    setSize(toggledConsoleSize(drawnSize(), reopenSize));
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

  sizeRecord = watchUiState(uiAddress({
    deviceId: context.deviceId || "",
    entityId: key,
    view: "console",
    kind: "fold",
  }), (saved) => {
    requestedSize = consoleSize(saved?.size);
    reopenSize = saved?.reopenSize === "full" ? "full" : DEFAULT_OPEN_SIZE;
    sizeKnown = true;
    if (!wantedHere || wantedApplied) paint();
  });
  void sizeRecord.ready.then(async () => {
    if (disposed) return;
    if (wantedHere) {
      wantedApplied = true;
      await sizeRecord.write({ size: DEFAULT_OPEN_SIZE, reopenSize: DEFAULT_OPEN_SIZE });
    } else {
      sizeKnown = true;
      paint();
    }
  });
  void takeUpCache();

  return {
    size: () => drawnSize(),
    toggle: () => toggleConsole(),
    dispose() {
      disposed = true;
      sizeRecord.dispose();
      document.removeEventListener("keydown", onKeydown);
      unwatchCache?.();
      unwatchCache = null;
      if (reconnectWatch) reconnectWatch.dispose();
      reconnectWatch = null;
      disposePane();
      host.innerHTML = "";
      delete host.dataset.size;
    },
  };
}
