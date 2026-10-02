// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute, takePushOpenMark, withDeviceOrResolve } from "./core/router.js";
import { renderInbox } from "./views/inbox.js";
import { renderBranch } from "./views/branchView.js";
import { renderWorkspace } from "./views/workspaceView.js";
import { renderProject } from "./views/projectView.js";
import { renderTask } from "./views/taskView.js";
import { renderTrackerTask } from "./views/trackerTaskView.js";
import { isSettingsRoute, renderSettingsModal } from "./views/settingsModal.js";
import { renderCaptureDecision } from "./views/captureDecision.js";
import { renderResolving } from "./views/resolving.js";
import { markConsoleTerminal } from "./core/consoleModel.js";
import { inboxRouteChanged } from "./core/inboxShell.js";
import { toolbarRouteChanged } from "./core/toolbar.js";
import { askToOpenLinkedAgent, standShell } from "./core/shell.js";
import { clearCacheScope } from "./core/cacheScope.js";
import { wipeCache } from "./core/localCache.js";
import { wipeUiRecords } from "./core/localUiStore.js";
import { routeChanged } from "./core/cacheSync.js";
import { canAnswer, contextFor, deviceContextIdentity, onDeviceStateChanged, resetDeviceContexts } from "./core/deviceContexts.js";
import { createViewingContext } from "./core/viewingContext.js";
import { forgetHomeFollow, forgetRendezvousSockets, forgetSecurityStops } from "./connection.js";
import { followTerminalDevice, resetTerminalManager, terminalDeviceId } from "./terminal/manager.js";
import { mountFocusMemory } from "./core/focusMemory.js";
import { resetDeviceFilterCache } from "./core/deviceFilter.js";
import { resetPendingPairing } from "./core/pendingPairing.js";
import { trackBridgeUpdateDevices } from "./core/bridgeUpdates.js";

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
  deviceFilter: null,

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
 *
 * THIS IS THE ONLY CALLER OF `wipeCache` (and `wipeUiRecords`), and there is deliberately no second
 * one for signing out. Signing out navigates away from this document: the tab
 * that would have to do the wiping is gone before it could, and the next
 * account arrives in a new document that calls this on its way in. A wipe
 * wired to a sign-out button would be a promise the product cannot keep —
 * a closed tab makes none of its calls — so the guarantee is made where it can
 * be: nothing of the previous account survives the reset that precedes the
 * next one.
 */
export function resetApplication() {
  App.accountEpoch += 1;
  trackBridgeUpdateDevices([]);
  settingsReturnRoute = { name: "inbox" };
  modalDispose?.();
  modalDispose = null;
  mountedRoute = null;
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
  resetDeviceFilterCache();
  App.deviceFilter = null;
  // A device the last account approved is nothing this one waits for.
  resetPendingPairing();
  clearCacheScope();
  // The cache is what the app paints from, so the previous account's board,
  // conversations and diffs go with its devices. Not awaited: the reset is
  // synchronous, and every address it could be read through is already dead.
  // Its unsent drafts and UI state are in their own store, and go too.
  void wipeCache();
  void wipeUiRecords();
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
  // What the reader is looking at belongs to the PAGE (#21 has the task page
  // name its task, so the project agent beside it knows which one is open), so
  // it is dropped when the page is replaced and not merely when the URL moves.
  // Opening a modal, and closing it again, is neither: the reader is still
  // looking at the page under the scrim, and that page does not read again on
  // the way back — so a clear here would be a clear for good.
  if (pageIsChanging(route)) App.viewingContext.clear();
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
  const { hash, fromPush } = takePushOpenMark(location.hash);
  if (fromPush) {
    // A notification opened this link: its first stand lands on the latest
    // message, and the URL keeps no trace of it (no new history entry).
    askToOpenLinkedAgent();
    history.replaceState(null, "", hash);
  }
  const route = routeFromHash(hash);
  if (route.term) markConsoleTerminal(route.term);
  return route;
}

/**
 * Route to a notification's deep link in this open window (push.js
 * `installPushOpenListener`), opening the conversation it names even where the
 * URL already names it and the rail was left collapsed or on another agent. A
 * link to the page already standing is stood again rather than waiting on a
 * hashchange that never comes. The stand this causes lands on the latest
 * message; the service worker's mark is dropped, so the URL never keeps it.
 */
export function followNotificationLink(markedHash) {
  const { hash } = takePushOpenMark(markedHash);
  askToOpenLinkedAgent();
  if (location.hash !== hash) {
    location.hash = hash;
    return;
  }
  if (!App.gated) standShell(App.route);
}

export function initRouter() {
  App.route = readRoute();
  onDeviceStateChanged(restandOverLandedMachine);
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
// project, a workspace, a branch, a task, a capture's decision page, or the
// holding screen a pre-redesign URL waits on. The account and a device's
// settings are not here: they are configuration rather than a place to stand,
// so they open as a modal over whichever of these the reader was on.
const VIEWS = {
  inbox: renderInbox,
  project: renderProject,
  branch: renderBranch,
  workspace: renderWorkspace,
  task: renderTask,
  trackerTask: renderTrackerTask,
  capture: renderCaptureDecision,
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
  disposeFocus?.();
  disposeFocus = null;
  App.routeLeaveGuard = null;
  mountedRoute = null;
  mountedIdentity = null;
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
// The page standing in #root, as the route that built it. Null when nothing is
// mounted; the answer to "is the reader already here?", which is what closing a
// modal has to ask before it rebuilds a page that never went away.
let mountedRoute = null;
// Which device context the mounted page was built over, as the registry names
// it. A retired machine or a new account mints another, and a page still
// holding the old one is standing over a machine the registry has let go of.
let mountedIdentity = null;
let disposeFocus = null;
// The open modal's teardown, which is NOT App.viewDispose: that slot belongs to
// the page underneath, and a modal that claimed it would tear that page down.
let modalDispose = null;

/** Whether taking up this route replaces the page in #root. A settings route
 *  lays a modal over the page instead, and a route the mounted page is already
 *  standing on is a modal closing back onto it. */
const pageIsChanging = (route) => !isSettingsRoute(route) && !sameRoute(mountedRoute, route);

/** Whether two routes name the same standing. Compared field by field rather
 *  than by JSON: a route that has been round-tripped through
 *  `withDeviceOrResolve` carries the same fields in whatever order that built
 *  them, and a key-order difference here would quietly rebuild a page that
 *  never went away. */
function sameRoute(a, b) {
  if (!a || !b) return false;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  return [...keys].every((key) => a[key] === b[key]);
}

/**
 * Settings, the account and a device's settings are configuration rather than
 * a place to stand, so they open OVER the page the reader was on.
 *
 * A URL may still name one for a deep link. Landing on one cold has no page
 * under it, so the page it will close back to is stood up first: closing lands
 * somewhere real, and the frame behind the scrim is never bare.
 */
function openSettingsOver() {
  if (!mountedRoute) {
    const opened = App.route;
    App.route = settingsReturnRoute;
    renderPage();
    App.route = opened;
  }
  if (modalDispose) return; // already open — the modal owns which section shows
  modalDispose = renderSettingsModal(settingsReturnRoute);
}

/** Shut the modal, and say whether there was one. */
function closeSettings() {
  if (!modalDispose) return false;
  modalDispose();
  modalDispose = null;
  return true;
}

/**
 * Stand the reader on App.route unless they already are.
 *
 * A session landing is news about the wire, not about the page (#170): the
 * page in #root painted from the cache and repaints when a write announces, so
 * the gate handing the app back after a reconnect leaves it where it is — its
 * nodes, its scroll and the reader's focus. It is built again only when it is
 * not the page for this route over this machine: nothing is mounted (a gate
 * screen had #root, or the account changed), the route moved, or the machine
 * it was built over was retired since.
 *
 * The terminals still follow the route's machine, which may be the one that
 * has just landed: a render would have taken them there.
 */
export function renderUnlessStanding() {
  if (!routeIsStanding()) {
    render();
    return;
  }
  followRouteDevice();
}

/**
 * The mounted route's machine was retired and has landed anew (#171): the page
 * and its shell were built over the context the registry let go of, so they
 * are built again over the one answering now, from the cache.
 *
 * Heard on its own rather than through the gate, which hands the app back only
 * when the last machine answers again: one machine retired while another stays
 * live never takes the app, so nothing else would rebuild the route. A gate
 * screen holding #root rebuilds it on the way out, and a machine that has not
 * landed yet leaves the page as it is.
 */
function restandOverLandedMachine() {
  if (App.gated || !mountedRoute || !sameRoute(mountedRoute, App.route)) return;
  const deviceId = App.route.deviceId;
  if (!deviceId || mountedIdentity === deviceContextIdentity(deviceId)) return;
  if (canAnswer(contextFor(deviceId))) render();
}

const routeIsStanding = () =>
  sameRoute(mountedRoute, App.route) && mountedIdentity === deviceContextIdentity(App.route.deviceId);

export function render() {
  if (isSettingsRoute(App.route)) {
    openSettingsOver();
    return;
  }
  // Closing a modal is not a navigation: the page under it never left, so it is
  // not built again around the same rail, the same reads and the same scroll.
  // Unless its machine was retired while the modal was open (#171): the page
  // under it stands over a context the registry has let go of.
  if (closeSettings() && routeIsStanding()) return;
  renderPage();
}

function renderPage() {
  unmountView();
  settingsReturnRoute = { ...App.route };
  inboxRouteChanged(); // keep the rail tracking the route
  toolbarRouteChanged(); // …and the toolbar naming where you are standing
  // …and the conversation rail and console standing on what this route is OF
  // (core/shell.js). Before the page paints, not after and not by the page: a
  // page that mounted its own rail could forget one, and the task page did.
  standShell(App.route);
  followRouteDevice(); // …and the terminals typing at the machine it names
  // The shell's grid owns the columns; #root is one cell. A view states its own
  // chrome (`surface` for a full-height work surface, nothing for a reading
  // page), so the outgoing view's never leaks into the incoming one.
  $("#root").className = "";
  mountedRoute = { ...App.route };
  mountedIdentity = deviceContextIdentity(App.route.deviceId);
  (VIEWS[App.route.name] || renderInbox)();
  disposeFocus = mountFocusMemory($("#root"), hashFromRoute(App.route), {
    deviceId: App.route.deviceId || "",
    entityId: App.route.workspaceId || App.route.branchId || App.route.projectId || App.route.id || "",
  });
}
