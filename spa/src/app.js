// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute, withDeviceOrResolve } from "./core/router.js";
import { renderInbox } from "./views/inbox.js";
import { renderBranch } from "./views/branchView.js";
import { renderIssue } from "./views/issueView.js";
import { renderDeviceSettings } from "./views/deviceSettings.js";
import { renderAccount } from "./views/account.js";
import { renderCaptureDecision } from "./views/captureDecision.js";
import { renderResolving } from "./views/resolving.js";
import { markConsoleTerminal } from "./core/consoleModel.js";
import { inboxRouteChanged } from "./core/inboxShell.js";
import { toolbarRouteChanged } from "./core/toolbar.js";
import { normalizeModelCatalog } from "./core/modelPicker.js";
import { clearCacheScope } from "./core/cacheScope.js";
import {
  adoptDeviceSession,
  homeContext,
  resetDeviceContexts,
  retireDeviceContext,
  setHomeContext,
} from "./core/deviceContexts.js";
import { createViewingContext } from "./core/viewingContext.js";

const SELECTED_DEVICE_KEY = "build.selectedDeviceId";

export const App = {
  call: null, // RPC into the live E2EE session (session.call)
  session: null, // { call, deviceId, close }
  cacheScope: null, // captured ownership of browser cache reads/writes
  chatRepository: null, // drafts/controllers owned by the current device scope
  viewingContext: createViewingContext({ enabled: false }),
  route: { name: "inbox" },
  poll: null, // the current view's change watcher (core/changeEvents.js)
  viewDispose: null, // the current view's teardown (terminal panes, observers)
  routeLeaveGuard: null, // async veto owned by the mounted view (for unsaved work)
  offline: false,
  offlineSince: null, // ms timestamp stamped by goOffline(), cleared on restore

  gated: true, // gate screens own #root until a session is live
  devices: [], // last GET /api/devices, statuses patched live by relay pushes
  selectedDeviceId: localStorage.getItem(SELECTED_DEVICE_KEY) || null,
  modelCatalog: null, // models.list result, fetched once per session

  // One-shot: set right before navigating to a branch just cut from the
  // toolbar's create form, so the branch view knows to focus the rail's
  // composer the moment it exists — read and cleared by the very next
  // renderBranch, which is what that navigation lands on. The route/hash
  // can't carry this itself (routeFromHash/hashFromRoute only round-trip
  // name/projectId/branch/tab).
  focusComposerOnMount: false,

};

// The compatibility aliases: the home device's context, copied onto App as
// plain fields. Plain, because tests and unmigrated surfaces assign App.call
// directly. Stage 3 deletes the six fields and everything below them here.
const ALIAS_DEFAULTS = Object.freeze({
  session: null,
  call: null,
  cacheScope: null,
  chatRepository: null,
  offline: false,
  offlineSince: null,
});

/** Say which context the App.* aliases — and creation — now follow. */
export function pointAliasesAt(context) {
  for (const [field, empty] of Object.entries(ALIAS_DEFAULTS)) App[field] = context?.[field] ?? empty;
  return setHomeContext(context);
}

/**
 * Adopt a session as the home device's: the registry creates or retargets that
 * device's context, and the aliases follow it. Reconnecting the same device
 * only replaces its transport.
 *
 * The compatibility shim, not the production path. Nothing in connection.js
 * calls it any more — moving home closes nothing, and every other device stays
 * live and keeps filling the inbox. What is left is the surface
 * adoptApplicationScope speaks through, and its retire-on-another-device is
 * what appScope.test.js pins. Stage 3 deletes both with that file.
 */
export function adoptHomeSession(session) {
  const previous = homeContext();
  const context = adoptDeviceSession(session);
  if (previous && previous !== context) retireSwitchedDevice(previous.deviceId);
  pointAliasesAt(context);
  return context;
}

// A device the shim above handed home to somebody else: it goes, with the model
// catalog and the reader's position it filled.
function retireSwitchedDevice(deviceId) {
  retireDeviceContext(deviceId);
  App.modelCatalog = null;
  App.viewingContext.clear();
}

/** Bind application chat state to a live device, given a session-shaped
 * { deviceId, call }. The registry is the real adoption; this is what the
 * callers that still speak in scopes say. */
export function adoptApplicationScope(session) {
  return adoptHomeSession(session).chatRepository;
}

/** Explicit auth/application teardown hook. The current product signs out by
 * leaving this document, but embedders and future in-place auth can call this
 * before replacing the account. */
export function disposeApplicationScope() {
  routeAttempt += 1;
  pendingLeaveDecision = null;
  App.routeLeaveGuard = null;
  App.viewingContext?.setEnabled?.(false);
  resetDeviceContexts();
  clearCacheScope();
  pointAliasesAt(null);
  App.modelCatalog = null;
}

/** The bridge's provider/model catalog, cached for the session.
 *  An older bridge without the RPC yields empty lists — selectors then offer
 *  only "Harness default", which is exactly what that bridge supports. */
export async function loadModelCatalog() {
  if (App.modelCatalog) return App.modelCatalog;
  const scope = App.cacheScope;
  const call = App.call;
  let catalog;
  try {
    catalog = normalizeModelCatalog(await call("models.list"));
  } catch {
    catalog = normalizeModelCatalog({ models: [], efforts: [] });
  }
  // A device switch can overtake this request. Its answer still belongs to the
  // caller that asked, but it must not become the catalog of the new scope.
  const stillOwned = scope ? scope === App.cacheScope && scope.active() : call === App.call;
  if (stillOwned) App.modelCatalog = catalog;
  return catalog;
}

export async function refreshModelCatalog() {
  App.modelCatalog = normalizeModelCatalog(await App.call("models.list"));
  return App.modelCatalog;
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
  return hashFromRoute(route);
}

function applyRoute(route) {
  App.routeLeaveGuard = null;
  const hash = standOn(route);
  if (location.hash !== hash) {
    acceptedHash = hash;
    location.hash = hash; // hashchange re-enters render()
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

// Every route is one of six surfaces: the inbox (the landing route), a branch,
// an issue, a capture's decision page, an account page, or the holding screen a
// pre-redesign URL waits on.
const VIEWS = {
  inbox: renderInbox,
  branch: renderBranch,
  issue: renderIssue,
  capture: renderCaptureDecision,
  account: renderAccount,
  device: renderDeviceSettings,
  resolve: renderResolving,
};

export function render() {
  if (App.poll) {
    App.poll.dispose();
    App.poll = null;
  }
  // Tear down the outgoing view's client-side resources (terminal panes, their
  // ResizeObservers + window listeners) before the next view claims #root. The
  // server PTYs persist — dispose never closes them.
  if (App.viewDispose) {
    try {
      App.viewDispose();
    } catch {
      /* a broken teardown must not block navigation */
    }
    App.viewDispose = null;
  }
  inboxRouteChanged(); // keep the rail tracking the route
  toolbarRouteChanged(); // …and the toolbar naming where you are standing
  // The shell's grid owns the columns; #root is one cell. A view states its own
  // chrome (`surface` for a full-height work surface, nothing for a reading
  // page), so the outgoing view's never leaks into the incoming one.
  $("#root").className = "";
  (VIEWS[App.route.name] || renderInbox)();
}
