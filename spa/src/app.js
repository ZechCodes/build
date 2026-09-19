// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute, withDeviceOrResolve } from "./core/router.js";
import { renderInbox } from "./views/inbox.js";
import { renderBranch } from "./views/branchView.js";
import { renderWorkspace } from "./views/workspaceView.js";
import { renderProject } from "./views/projectView.js";
import { renderIssue } from "./views/issueView.js";
import { isSettingsRoute, renderSettingsModal } from "./views/settingsModal.js";
import { renderAccount } from "./views/account.js";
import { renderCaptureDecision } from "./views/captureDecision.js";
import { renderResolving } from "./views/resolving.js";
import { markConsoleTerminal } from "./core/consoleModel.js";
import { inboxRouteChanged } from "./core/inboxShell.js";
import { toolbarRouteChanged } from "./core/toolbar.js";
import { clearCacheScope } from "./core/cacheScope.js";
import { wipeCache } from "./core/localCache.js";
import { routeChanged } from "./core/cacheSync.js";
import { resetDeviceContexts } from "./core/deviceContexts.js";
import { createViewingContext } from "./core/viewingContext.js";
import { forgetHomeFollow, forgetRendezvousSockets, forgetSecurityStops } from "./connection.js";
import { followTerminalDevice, resetTerminalManager, terminalDeviceId } from "./terminal/manager.js";

const SELECTED_DEVICE_KEY = "build.selectedDeviceId";
// Which machines the rail lists (core/deviceFilter.js). Minted here, beside the
// pick, because this is where both are read off the browser at boot.
export const DEVICE_FILTER_KEY = "build.deviceFilter";

// What the app holds that is nobody's machine in particular: where the reader
// is standing, what the mounted view owes a teardown, and the account's device
// list. Everything that belongs to a machine — its transport, its cache scope,
// its drafts, whether it can answer — is held on that machine's context
// (core/deviceContexts.js), because the app is on all of them at once.
export const App = {
  accountEpoch: 0,
  viewingContext: createViewingContext({ enabled: false }),
  route: { name: "inbox" },
  poll: null, // the current view's change watcher (core/changeEvents.js)
  viewDispose: null, // the current view's teardown (terminal panes, observers)
  routeLeaveGuard: null, // async veto owned by the mounted view (for unsaved work)

  gated: true, // gate screens own #root until a session is live
  updateAvailable: false, // the served-version watcher found a newer bundle than this one
  devices: [], // last GET /api/devices, re-read by the presence poll (devices.js)
  selectedDeviceId: localStorage.getItem(SELECTED_DEVICE_KEY) || null,
  // Which machines the inbox, the projects face and the project menu list —
  // null for all of them. It narrows lists and nothing else: no route, no
  // session and no creation reads it.
  deviceFilter: localStorage.getItem(DEVICE_FILTER_KEY) || null,

  // One-shot: set right before navigating to a branch just cut from the
  // toolbar's create form, so the branch view knows to focus the rail's
  // composer the moment it exists — read and cleared by the very next
  // renderBranch, which is what that navigation lands on. The route/hash
  // can't carry this itself (routeFromHash/hashFromRoute only round-trip
  // name/projectId/branch/tab).
  focusComposerOnMount: false,

};

/**
 * Put the application back to its just-loaded state: no route guard, no route
 * attempt in flight, nobody being read, no device open, no terminals on any
 * machine, no cache addressed and nothing left on disk from the account that
 * was here.
 *
 * The current product signs out by leaving this document, so nothing in the
 * running app calls this — it is what an embedder or future in-place auth
 * would call before replacing the account, and what a suite calls between
 * cases so one test's devices cannot answer the next one's reads.
 */
export function resetApplication() {
  App.accountEpoch += 1;
  settingsReturnRoute = { name: "inbox" };
  routeAttempt += 1;
  pendingLeaveDecision = null;
  App.routeLeaveGuard = null;
  App.viewingContext?.setEnabled?.(false);
  // Invalidate attempt authority before retiring contexts: a late greeting or
  // mint belongs to the old account and cannot land while teardown runs.
  resetTerminalManager();
  forgetRendezvousSockets();
  resetDeviceContexts();
  // The terminals are on nobody now, so the next route that names a device is a
  // move however familiar the name, and home has been followed for nobody.
  terminalRouteDeviceId = null;
  forgetHomeFollow();
  forgetSecurityStops();
  clearCacheScope();
  // The cache is what the app paints from, so the previous account's board,
  // conversations and diffs go with its devices. Not awaited: the reset is
  // synchronous, and every address it could be read through is already dead.
  void wipeCache();
}

export function rememberSelectedDevice(deviceId) {
  App.selectedDeviceId = deviceId;
  if (deviceId) localStorage.setItem(SELECTED_DEVICE_KEY, deviceId);
  else localStorage.removeItem(SELECTED_DEVICE_KEY);
}

let acceptedHash = null;
let routeAttempt = 0;
let pendingLeaveDecision = null;

function mayLeaveRoute() {
  const guard = App.routeLeaveGuard;
  if (!guard) return Promise.resolve(true);
  if (!pendingLeaveDecision || pendingLeaveDecision.guard !== guard) {
    const pending = { guard, decision: null };
    pending.decision = Promise.resolve(guard())
      .catch(() => false)
      .finally(() => {
        if (pendingLeaveDecision === pending) pendingLeaveDecision = null;
      });
    pendingLeaveDecision = pending;
  }
  return pendingLeaveDecision.decision;
}

/**
 * Take up a route: hold it, and answer with the link that names it.
 *
 * The route as given is what the URL says; the route the app stands on is that
 * route once it names a machine, or the resolve hop that finds one. A parked
 * route has no hash of its own, so the link is written from the route as given.
 */
function standOn(route) {
  App.viewingContext.clear();
  App.route = withDeviceOrResolve(route);
  // Every route the app takes up comes through here — a navigation and a move
  // within a surface alike — and either can change which workspace is on
  // screen. The sync layer's realtime subscription follows it (core/cacheSync).
  routeChanged();
  return hashFromRoute(route);
}

function applyRoute(route) {
  App.routeLeaveGuard = null;
  const hash = standOn(route);
  if (location.hash !== hash) {
    // Replace keeps reloads and shared URLs useful without accumulating history.
    acceptedHash = hash;
    location.replace(hash); // hashchange re-enters render after this view settles
  } else {
    acceptedHash = null;
    render();
  }
}

export function go(route) {
  const attempt = ++routeAttempt;
  const guard = App.routeLeaveGuard;
  if (!guard) {
    applyRoute(route);
    return true;
  }
  return mayLeaveRoute().then((allowed) => {
    const current = allowed && attempt === routeAttempt;
    if (current) applyRoute(route);
    return current;
  });
}

/**
 * Record where the reader is standing, without re-rendering.
 *
 * A surface that moves WITHIN itself — the Files tab opening another file —
 * still owes the URL an answer, so the link stays sendable and a reload lands
 * in the same place. Going through `go` would tear the surface down and build
 * it again around the same tab, refetching everything it already holds and
 * losing the reader's place in it.
 */
export function markRoute(route) {
  const hash = standOn(route);
  if (location.hash === hash) return;
  history.replaceState(null, "", hash);
}

/** The route the URL names.
 *
 *  A pre-redesign `term-<n>` URL also names a terminal, and the canonical URL
 *  it rewrites to has nowhere to keep it — the console is not a tab any more.
 *  So it is handed to the console here, at the one place every URL is read,
 *  and the next console to mount opens on it. */
function readRoute() {
  const route = routeFromHash(location.hash);
  if (route.term) markConsoleTerminal(route.term);
  return route;
}

export function initRouter() {
  App.route = readRoute();
  document.addEventListener("click", followRouteLink);
  window.addEventListener("hashchange", async () => {
    const requestedHash = location.hash;
    if (acceptedHash === requestedHash) {
      acceptedHash = null;
      App.route = readRoute();
      if (!App.gated) render();
      return;
    }
    const attempt = ++routeAttempt;
    const allowed = await mayLeaveRoute();
    if (attempt !== routeAttempt || location.hash !== requestedHash) return;
    if (!allowed) {
      history.replaceState(null, "", hashFromRoute(App.route));
      return;
    }
    App.viewingContext.clear();
    App.route = readRoute();
    if (!App.gated) render();
  });
}

function followRouteLink(event) {
  if (event.defaultPrevented || event.button !== 0) return;
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  const link = event.target.closest?.("a[href]");
  const hash = routeLinkHash(link);
  if (!hash) return;
  event.preventDefault();
  void go(routeFromHash(hash));
}

function routeLinkHash(link) {
  if (!link || link.hasAttribute("download") || (link.target && link.target !== "_self")) return;
  const url = new URL(link.href, location.href);
  if (url.origin !== location.origin || url.pathname !== location.pathname || url.search !== location.search) return;
  return url.hash.startsWith("#/") ? url.hash : null;
}

// Every route is one of these surfaces: the inbox (the landing route), a
// project, a workspace, a branch, an issue, a capture's decision page, an
// account page, or the holding screen a pre-redesign URL waits on.
const VIEWS = {
  inbox: renderInbox,
  project: renderProject,
  branch: renderBranch,
  workspace: renderWorkspace,
  issue: renderIssue,
  capture: renderCaptureDecision,
  account: renderAccount,
  resolve: renderResolving,
};

// The device the terminals were last taken to for a route. A route change is
// one of the two ways the machine the shells type at moves (a home move is the
// other), and only a change: re-pointing a socket that is already on the right
// device would drop every open tab for nothing.
let terminalRouteDeviceId = null;

function followRouteDevice() {
  // A link still being looked up names no machine yet and mounts no surface:
  // taking the terminals home for that beat and back again when the feed answers
  // would drop the socket twice over one navigation.
  if (App.route.name === "resolve") return;
  const deviceId = terminalDeviceId();
  if (deviceId === terminalRouteDeviceId) return;
  // A machine that cannot answer takes nothing: the shells stay where they are,
  // and this stays unrecorded so the next render — the one after that machine
  // lands — takes them.
  if (followTerminalDevice()) terminalRouteDeviceId = deviceId;
}

/**
 * Take the mounted view down: its read, and its own client-side resources
 * (terminal panes, their ResizeObservers and window listeners).
 *
 * The server PTYs persist — dispose never closes them. Every render runs this
 * before the next view claims #root, and so does the gate when it takes #root
 * back from a view whose machine has gone.
 *
 * The route guard goes with the view that set it. Left standing, it is asked
 * again for the navigation the NEXT view makes — the resolve hop moving on to
 * the work item it just found — and a veto there strands the reader on a
 * holding screen with nowhere to go.
 */
export function unmountView() {
  App.routeLeaveGuard = null;
  if (App.poll) {
    App.poll.dispose();
    App.poll = null;
  }
  if (App.viewDispose) {
    try {
      App.viewDispose();
    } catch {
      /* a broken teardown must not block navigation */
    }
    App.viewDispose = null;
  }
}

let settingsReturnRoute = { name: "inbox" };

export function render() {
  unmountView();
  if (isSettingsRoute(App.route)) {
    renderSettingsModal(settingsReturnRoute);
    return;
  }
  settingsReturnRoute = { ...App.route };
  inboxRouteChanged(); // keep the rail tracking the route
  toolbarRouteChanged(); // …and the toolbar naming where you are standing
  followRouteDevice(); // …and the terminals typing at the machine it names
  // The shell's grid owns the columns; #root is one cell. A view states its own
  // chrome (`surface` for a full-height work surface, nothing for a reading
  // page), so the outgoing view's never leaks into the incoming one.
  $("#root").className = "";
  (VIEWS[App.route.name] || renderInbox)();
}
