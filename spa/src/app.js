// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute } from "./core/router.js";
import { renderInbox } from "./views/inbox.js";
import { renderBranch } from "./views/branchView.js";
import { renderIssue } from "./views/issueView.js";
import { renderAccount } from "./views/account.js";
import { renderCaptureDecision } from "./views/captureDecision.js";
import { renderResolving } from "./views/resolving.js";
import { markConsoleTerminal } from "./core/consoleModel.js";
import { inboxRouteChanged } from "./core/inboxShell.js";
import { toolbarRouteChanged } from "./core/toolbar.js";
import { normalizeModelCatalog } from "./core/modelPicker.js";

const SELECTED_DEVICE_KEY = "build.selectedDeviceId";

export const App = {
  call: null, // RPC into the live E2EE session (session.call)
  session: null, // { call, deviceId, close }
  route: { name: "inbox" },
  poll: null, // the current view's change watcher (core/changeEvents.js)
  viewDispose: null, // the current view's teardown (terminal panes, observers)
  offline: false,
  offlineSince: null, // ms timestamp stamped by goOffline(), cleared on restore

  gated: true, // gate screens own #root until a session is live
  devices: [], // last GET /api/devices, statuses patched live by relay pushes
  selectedDeviceId: localStorage.getItem(SELECTED_DEVICE_KEY) || null,
  modelCatalog: null, // models.list result, fetched once per session
};

/** The bridge's provider/model catalog, cached for the session.
 *  An older bridge without the RPC yields empty lists — selectors then offer
 *  only "Harness default", which is exactly what that bridge supports. */
export async function loadModelCatalog() {
  if (App.modelCatalog) return App.modelCatalog;
  try {
    App.modelCatalog = normalizeModelCatalog(await App.call("models.list"));
  } catch {
    App.modelCatalog = normalizeModelCatalog({ models: [], efforts: [] });
  }
  return App.modelCatalog;
}

export function rememberSelectedDevice(deviceId) {
  App.selectedDeviceId = deviceId;
  if (deviceId) localStorage.setItem(SELECTED_DEVICE_KEY, deviceId);
  else localStorage.removeItem(SELECTED_DEVICE_KEY);
}

export function go(route) {
  App.route = route;
  const hash = hashFromRoute(route);
  if (location.hash !== hash) location.hash = hash; // hashchange re-enters render()
  else render();
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
  window.addEventListener("hashchange", () => {
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
