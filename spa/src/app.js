// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute } from "./core/router.js";
import { loadReadIds, persistReadIds } from "./core/readState.js";
import { renderNotifications } from "./views/notifications.js";
import { renderSettings } from "./views/settings.js";
import { renderTask } from "./views/task.js";
import { renderPlan } from "./views/plan.js";
import { renderWorktree } from "./views/worktree.js";
import { renderMain } from "./views/mainWorktree.js";
import { renderProject } from "./views/project.js";
import { sidebarRouteChanged } from "./views/sidebar.js";

const SELECTED_DEVICE_KEY = "build.selectedDeviceId";

export const App = {
  call: null, // RPC into the live E2EE session (session.call)
  session: null, // { call, deviceId, close }
  route: { name: "notifications" },
  poll: null,
  viewDispose: null, // the current view's teardown (terminal panes, observers)
  readIds: loadReadIds(localStorage), // persisted; pruned against the feed each tick
  offline: false,
  gated: true, // gate screens own #root until a session is live
  devices: [], // last GET /api/devices, statuses patched live by relay pushes
  selectedDeviceId: localStorage.getItem(SELECTED_DEVICE_KEY) || null,
  modelCatalog: null, // models.list result, fetched once per session
};

/** The bridge's model catalog ({models, efforts}), cached for the session.
 *  An older bridge without the RPC yields empty lists — selectors then offer
 *  only "Harness default", which is exactly what that bridge supports. */
export async function loadModelCatalog() {
  if (App.modelCatalog) return App.modelCatalog;
  try {
    App.modelCatalog = await App.call("models.list");
  } catch {
    App.modelCatalog = { models: [], efforts: [] };
  }
  return App.modelCatalog;
}

/** Mark one run/plan read (visiting its surface counts as reading it) and
 *  persist. Badges pick the change up on the next feed tick. */
export function markEntityRead(id) {
  if (!id || App.readIds.has(id)) return;
  App.readIds.add(id);
  persistReadIds(App.readIds, localStorage);
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

export function initRouter() {
  App.route = routeFromHash(location.hash);
  window.addEventListener("hashchange", () => {
    App.route = routeFromHash(location.hash);
    if (!App.gated) render();
  });
}

export function setActiveNav(name) {
  const target = { notifications: "#nav-notif", settings: "#nav-account" }[name] || null;
  ["#nav-notif", "#nav-account"].forEach((s) => {
    const row = $(s);
    if (row) row.classList.toggle("active", s === target);
  });
}

export function render() {
  if (App.poll) {
    clearInterval(App.poll);
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
  setActiveNav(App.route.name);
  sidebarRouteChanged(); // keep the rail's active row tracking the route
  // Worktree-backed surfaces (task/worktree/main) are full-height tab shells;
  // every other view keeps the centered reading column.
  const surfaceRoutes = ["task", "worktree", "main", "plan"];
  $("#root").classList.toggle("surface", surfaceRoutes.includes(App.route.name));
  if (App.route.name === "notifications") renderNotifications();
  else if (App.route.name === "settings") renderSettings();
  else if (App.route.name === "worktree") renderWorktree();
  else if (App.route.name === "main") renderMain();
  else if (App.route.name === "project") renderProject();
  else if (App.route.name === "plan") renderPlan();
  else renderTask();
}
