// App shell: shared state, the hash router wiring, and render dispatch.

import { $ } from "./dom.js";
import { routeFromHash, hashFromRoute } from "./core/router.js";
import { renderBoard } from "./views/board.js";
import { renderNotifications } from "./views/notifications.js";
import { renderSettings } from "./views/settings.js";
import { renderTask } from "./views/task.js";
import { renderWorktree } from "./views/worktree.js";
import { sidebarRouteChanged } from "./views/sidebar.js";

const SELECTED_DEVICE_KEY = "build.selectedDeviceId";

export const App = {
  call: null, // RPC into the live E2EE session (session.call)
  session: null, // { call, deviceId, close }
  route: { name: "board" },
  poll: null,
  readIds: new Set(),
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
  const target = { board: "#nav-board", notifications: "#nav-notif", settings: "#nav-settings" }[name] || "#nav-board";
  ["#nav-board", "#nav-notif", "#nav-settings"].forEach((s) => $(s).classList.toggle("active", s === target));
}

export function render() {
  if (App.poll) {
    clearInterval(App.poll);
    App.poll = null;
  }
  setActiveNav(App.route.name);
  sidebarRouteChanged(); // keep the rail's active row tracking the route
  if (App.route.name === "board") renderBoard();
  else if (App.route.name === "notifications") renderNotifications();
  else if (App.route.name === "settings") renderSettings();
  else if (App.route.name === "worktree") renderWorktree();
  else renderTask();
}
