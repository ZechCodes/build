// The view area's toolbar: where you are standing, how to go somewhere else,
// and what the work you are standing in is doing.
//
// Left: independent project ancestry and local workspace selection. The up
// arrow opens the existing global project list; direct global destinations and
// visit ordering belong to its next migration. Both searches mount and focus
// inside the activating gesture, with persistence following the interaction.
//
// Right: a slot the standing view can fill with its own verb — a branch's
// Done, say. The working-time ticker
// and the diffstat used to sit beside it too; they pin above the agent rail's
// composer now instead (core/agentRail.js) — a fact about the work item, read
// beside the conversation about it rather than in a bar that outlives every
// view.
//
// The toolbar outlives views (it is the shell's row, not a view's), so it
// mounts once and repaints from the feed and the route. The verb slot is the
// one piece of it a view owns: `setToolbarVerb`/`clearToolbarVerb` register
// and release a paint function the view supplies, called every repaint —
// including poll-driven ones the toolbar itself would otherwise skip, since a
// verb like Done carries its own in-flight/open-menu state that must survive
// a tick it has nothing new to say.

import { $ } from "../dom.js";
import { esc } from "./text.js";
import { App, go } from "../app.js";
import { subscribeFeed } from "./taskFeed.js";
import { notifyError } from "./notify.js";
import { openCreateWork } from "./createWork.js";
import { projectMenuModel, toolbarIdentity, toolbarDestinationModel } from "./toolbarModel.js";
import { deviceTagHtml, projectNameOf } from "./inboxProjects.js";
import { filterByDevice } from "./deviceFilter.js";
import { deviceKey, routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { deviceView } from "./feedMerge.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { patchList } from "./patchList.js";
import { toolbarHtml, unreadBadgeHtml } from "./toolbarRender.js";
import { navigationSelectionTarget, projectReturnTarget } from "./navigationRoutes.js";
import { hashFromRoute, routeFromHash } from "./router.js";
import { mountMeasuredHeight } from "./measuredInset.js";
import { taskAddress } from "./trackerCache.js";
import { standsOnProjectCheckout } from "./workspaceModel.js";
import "../styles/shell.css";
import { workspaceLockState, paintWorkspaceLock } from "./workspaceLock.js";
import { watchWorkspaceLockSupport } from "./workspaceLockSupport.js";
import "../styles/workspace-lock.css";
import { fieldTraits } from "./fieldTraits.js";
import { readCached, subscribeCache } from "./localCache.js";
import { NO_REVIEW_SUPPORT, readReviewSupport, reviewSupportAddress, reviewSupportFor } from "./taskReviewSupport.js";

const SCOPE_ADDRESS = uiAddress({ view: "toolbar", kind: "filter", sub: "project" });
const MENU_ADDRESS = uiAddress({ view: "toolbar", kind: "menu", sub: "jump" });

// Every machine's rows: where you are standing, and what the project menu lists.
let feed = { items: [], projects: [] };
// The same snapshot narrowed to the machines the picker is showing. Only the
// list of places to GO is narrowed — the bar names the project you are in
// whether or not the filter shows it.
let shownFeed = feed;
// The account-wide name of the scoped project (core/deviceKey.js), never the
// bare id: every machine mints a `proj-1`, and the menu lists them all.
let scopedProjectKey = null;
// Each project's workspaces as its own machine last listed them, by project
// key — two machines' `proj-1` are two projects with two sets.
const workspacesByProject = new Map();
let open = null; // { element, anchor, mode, dismiss } while the menu is up
let mounted = false;
let paintedIdentity = null; // what the bar's OWN markup was last built from — see paint()
let verbRender = null; // (host) => void, the standing view's own verb-slot paint
let scopeRecord = null;
let menuRecord = null;
let scopeReady = false;
let pendingMenuFocus = false;
let cachedMenuValue = null;
let unsubscribeFeed = null;
let toolbarReady = Promise.resolve();
let toolbarRun = 0;
let lockDeviceId = null;
let lockSupported = false;
let stopLockSupport = null;
let reviewDeviceId = null;
let reviewSupport = NO_REVIEW_SUPPORT;
let stopReviewSupport = null;
let stopToolbarMeasurement = null;
let taskKey = null;
let taskLabel = null;
let stopTaskLabel = null;
let returnRead = 0;

function watchStandingReviewSupport() {
  const deviceId = App.route.name === "workspace" ? App.route.deviceId : null;
  if (deviceId === reviewDeviceId) return;
  stopReviewSupport?.();
  reviewDeviceId = deviceId;
  reviewSupport = NO_REVIEW_SUPPORT;
  if (!deviceId) return;
  let disposed = false;
  let reading = 0;
  const read = async () => {
    const token = ++reading;
    const support = await readReviewSupport(deviceId);
    if (disposed || token !== reading) return;
    reviewSupport = support;
    paint();
  };
  const stop = subscribeCache(reviewSupportAddress(deviceId), () => void read());
  stopReviewSupport = () => { disposed = true; stop(); };
  void read();
}

function watchStandingLockSupport() {
  const deviceId = App.route.name === "workspace" ? App.route.deviceId : null;
  if (deviceId === lockDeviceId) return;
  stopLockSupport?.();
  lockDeviceId = deviceId;
  lockSupported = false;
  stopLockSupport = deviceId ? watchWorkspaceLockSupport(deviceId, (supported) => {
    lockSupported = supported;
    paint();
  }) : null;
}

function standingWorkspace() {
  return (workspacesByProject.get(routeProjectKey(App.route)) || [])
    .find((row) => row.id === App.route.workspaceId);
}

function standingWorkspaceLock() {
  const workspace = standingWorkspace();
  return workspaceLockState(workspace && { ...workspace, deviceId: App.route.deviceId }, lockSupported);
}


/** Register the standing view's verb-slot content — called every repaint the
 *  toolbar does, poll-driven ticks included, so the caller's own function must
 *  be idempotent the way core/views/branchView.js's `paintFinish` already is
 *  (skip when nothing changed, freeze while a menu is open or an action is in
 *  flight). Repaints immediately so the slot fills without waiting for the
 *  next tick. */
export function setToolbarVerb(render) {
  verbRender = render;
  paintVerb();
}

/** Release the verb slot — a view calls this from its own teardown. Only the
 *  view that set it can clear it: an async clear racing a newer view's set
 *  (a fast back-to-back navigation) must not blank a slot that is no longer
 *  this caller's to own. */
export function clearToolbarVerb(render) {
  if (verbRender !== render) return;
  verbRender = null;
  paintVerb();
}

function paintVerb() {
  const host = $("#tb-verb");
  if (!host) return;
  if (verbRender) verbRender(host);
  else host.innerHTML = "";
}

/** The projects the menu offers to move to: every machine's, or one machine's
 *  while the picker is filtering the rail. */
const projectsOf = () => shownFeed.projects || [];
/** A project by its account-wide name, from every machine's — the project you
 *  are standing in is yours to name whether or not the menu is listing it. */
const projectFor = (projectKey) => (feed.projects || []).find((project) => project.projectKey === projectKey) || null;
/** What a project is called (core/inboxProjects.js), and "" for no project at
 *  all. */
const nameOf = (project) => (project ? projectNameOf(project) : "");

/** The project the menu is scoped to: where you are standing, else the last
 *  place you stood, else the first project the account knows. */
function scopedProject() {
  return scopedProjectKey ? projectFor(scopedProjectKey) : projectsOf()[0] || null;
}

function rememberScope(projectKey) {
  if (!projectKey || projectKey === scopedProjectKey) return Promise.resolve();
  const saved = scopeRecord ? scopeRecord.write({ projectKey }) : Promise.resolve();
  scopedProjectKey = projectKey;
  return saved;
}

/** One project's workspaces, off the cache the feed is a view over. The rows
 *  are already stamped with the machine they are on (core/feedMerge.js), so a
 *  row the bar lists and a row the rail lists are the same row.
 *
 *  Whole rows, directory tabs and all: the daemon writes one workspace shape
 *  (`workspace_json`) and `workspace.list` and `workspace.get` both answer in
 *  it, so the list is not a summary of the read this used to make beside it.
 *  A bridge that ever summarized its list would have to say so in its greeting
 *  — a tab strip quietly emptying is not something to read a workspace back
 *  one at a time against.
 *
 *  The project's own checkout is never a place to go
 *  (core/workspaceModel.js), whatever an older bridge on that machine listed. */
function workspaceRows(deviceId, projectId) {
  const project = projectFor(deviceKey(deviceId, projectId));
  return (deviceView(feed, deviceId).workspaces || [])
    .filter((workspace) => workspace.project_id === projectId)
    .filter((workspace) => !standsOnProjectCheckout(workspace, project));
}

/** Take up the workspace menu for one project. Nothing is read: the pass and
 *  the board pushes fill the workspace list, and this is the bar's own view of
 *  it. A project whose machine has never been read lists nothing, and lists
 *  something the moment that machine's records land. */
function loadWorkspaces(project, { repaint = true } = {}) {
  if (!project?.projectKey) return;
  workspacesByProject.set(project.projectKey, workspaceRows(project.deviceId, project.id));
  if (repaint) paint();
}

/** The project the route is standing in, as the workspace reads need it: the
 *  route is the authority on which machine and which project, whether or not
 *  the feed has arrived yet. */
const routeProject = () => ({
  id: App.route.projectId,
  deviceId: App.route.deviceId,
  projectKey: routeProjectKey(App.route),
});

/** Read the workspaces of the project the route is standing in, when the route
 *  is standing in a workspace at all. Every way into a route runs this. */
const loadStandingWorkspaces = () => {
  if (App.route.name === "workspace") loadWorkspaces(routeProject());
};

// ---- the bar ----------------------------------------------------------------

function identity() {
  return toolbarIdentity(App.route, {
    ...feed,
    workspaces: workspacesByProject.get(routeProjectKey(App.route)) || [],
  });
}

/** Repaint the bar. `entering` says the paint follows a navigation (a route the
 *  shell just entered, or the mount); every other paint is a poll landing.
 *
 *  Only a navigation re-scopes the menu. A poll must not: while you stand on one
 *  project's branch you can pick another project in the switcher, and a feed
 *  tick two seconds later would otherwise re-derive the scope from the route you
 *  are still standing on and hand the menu back to that project — the pick would
 *  never survive long enough to be acted on. Your pick stands until you move. */
function paint({ entering = false } = {}) {
  watchStandingLockSupport();
  watchStandingReviewSupport();
  watchTaskLabel();
  const host = $("#toolbar");
  if (!host || !scopeReady) return;
  const standing = identity();
  // Navigating into a work item scopes the menu to its project — the toolbar
  // reads as one sentence, so the two halves can never name different projects.
  // A route that names a project but no work item scopes it too: that is where
  // a project link lands now, since a project's own checkout is not a surface
  // (core/router.js).
  if (entering) scopeToRoute(standing.projectKey || routeProjectKey(App.route));
  const shown = shownIdentity(standing);
  drawToolbar(host, shown, entering);

  paintWorkspaceLock(host, shown.workspaceLock, () => { if (mounted) paint(); });
  paintVerb();
  if (open) paintMenu();
  updateProjectReturnLink(host);
}

function scopeToRoute(key) {
  if (!key) return;
  void rememberScope(key);
  scopedProjectKey = key;
}

function drawToolbar(host, shown, entering) {
  const signature = JSON.stringify({ ...shown, workspaceLock: null });
  if (!entering && signature === paintedIdentity && host.querySelector(".toolbar")) return;
  paintedIdentity = signature;
  host.innerHTML = toolbarHtml(shown);
  wireToolbar(host);
  stopToolbarMeasurement?.();
  stopToolbarMeasurement = mountMeasuredHeight(host, host.closest("#view"), "--toolbar-h");
}

/** What the bar draws for where the route stands: the project (the scoped one
 *  where the route names none), the kind and the work item's label. */
const currentHash = () => {
  const hash = window.location.hash;
  return hash && hashFromRoute(routeFromHash(hash)) === hashFromRoute(App.route) ? hash : hashFromRoute(App.route);
};

const shownIdentity = (standing) => ({
  project: standing.project,
  kind: standing.kind,
  label: standing.label,
  route: App.route,
  currentHash: currentHash(),
  task: taskLabel,
  deviceTag: projectDeviceTag(standing.projectKey),
  workspaceLock: standing.kind === "workspace" ? standingWorkspaceLock() : null,
});

function projectDeviceTag(key) {
  const group = toolbarDestinationModel(App.route, feed, { devices: App.devices }).groups
    .find((group) => group.projectKey === key);
  return group?.clash ? group.deviceName || group.deviceId : "";
}

function watchTaskLabel() {
  const route = App.route;
  const address = route.taskId ? taskAddress(route.deviceId, route.projectId, route.taskId) : null;
  const key = address ? JSON.stringify(address) : null;
  if (key === taskKey) return;
  stopTaskLabel?.();
  taskKey = key;
  taskLabel = null;
  stopTaskLabel = null;
  if (!address) return;
  let disposed = false;
  let reading = 0;
  const read = async () => {
    const token = ++reading;
    const record = await readCached(address);
    if (disposed || token !== reading) return;
    taskLabel = record?.value?.task ? { number: record.value.task.number } : null;
    paint();
  };
  const stop = subscribeCache(address, () => void read());
  stopTaskLabel = () => { disposed = true; stop(); };
  void read();
}

const standingProject = () => ({ id: App.route.projectId, deviceId: App.route.deviceId });
const plainActivation = (event) => !event.ctrlKey && !event.metaKey && !event.shiftKey && !event.altKey && event.button === 0;

function wireToolbar(host) {
  if (open) {
    open.anchor = host.querySelector(`[data-select="${open.select}"]`) || open.anchor;
    open.anchor.setAttribute("aria-expanded", "true");
    positionMenu(open.element, open.anchor);
  }
  for (const control of host.querySelectorAll("[data-select]")) control.onclick = (event) => {
    event.stopPropagation();
    activateSwitcher(control);
  };
  const project = host.querySelector("[data-project-return]");
  if (project) project.onclick = (event) => {
    if (!plainActivation(event)) return;
    event.preventDefault();
    void openProjectPage(standingProject());
  };
  for (const link of host.querySelectorAll('[data-tasks-parent]')) link.onclick = (event) => {
    if (!plainActivation(event)) return;
    event.preventDefault();
    go(routeFromHash(link.getAttribute("href")));
  };
  for (const link of host.querySelectorAll('.tb-breadcrumbs a[aria-current="page"]')) link.onclick = (event) => {
    if (plainActivation(event)) event.preventDefault();
  };
}

function activateSwitcher(control) {
  if (open?.anchor === control) return closeMenu();
  removeMenu(false);
  const list = MENU_FOR_SELECTOR[control.dataset.select];
  if (list === "workspaces") {
    void rememberScope(routeProjectKey(App.route) || scopedProjectKey);
    loadWorkspaces(scopedProject(), { repaint: false });
  }
  // Focus must precede IndexedDB, imports or animations to summon a touch keyboard.
  openJumpMenu(control, { list, focus: true });
  pendingMenuFocus = false;
  persistMenu({ open: true, select: control.dataset.select, list, query: "" });
}

function updateProjectReturnLink(host) {
  const link = host.querySelector("[data-project-return]");
  if (!link) return;
  const read = ++returnRead;
  const route = App.route;
  void projectReturnTarget(standingProject(), { route, currentHash: currentHash() }).then((target) => {
    if (read === returnRead && link.isConnected && App.route === route) link.href = target.hash;
  });
}

/** Explicit project ancestry returns to its saved face; a self-link does nothing. */
async function openProjectPage(project = returnProject()) {
  const standing = App.route;
  closeMenu();
  if (!project) return;
  const target = await projectReturnTarget(project, { route: standing, currentHash: currentHash() });
  if (App.route === standing && !target.current) go(target.route);
}

// ---- the menu each selector opens -------------------------------------------

function positionMenu(element, anchor) {
  const box = anchor.getBoundingClientRect();
  const viewport = window.visualViewport;
  const offset = viewport?.offsetTop || 0;
  const bottom = offset + (viewport?.height || window.innerHeight);
  const top = Math.max(offset + 8, Math.min(box.bottom + 4, bottom - 180));
  element.style.position = "fixed";
  element.style.left = `${Math.max(8, Math.min(box.left, window.innerWidth - 340))}px`;
  element.style.top = `${top}px`;
  element.style.setProperty("--tbmenu-max-h", `${bottom - top - 8}px`);
}

function menuShell(anchor, className) {
  const element = document.createElement("div");
  element.className = className;
  element.setAttribute("role", "dialog");
  element.setAttribute("aria-label", "Switch project or workspace");
  // Fixed to the viewport under its anchor, like the tab row's menus: the
  // toolbar is a narrow row and would clip a child popup.
  document.body.appendChild(element);
  const position = () => positionMenu(element, open?.element === element ? open.anchor : anchor);
  position();
  window.visualViewport?.addEventListener("resize", position);
  window.visualViewport?.addEventListener("scroll", position);
  window.addEventListener("resize", position);

  const onKeydown = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopImmediatePropagation();
      closeMenu({ restoreFocus: true });
    }
  };
  const onOutside = (event) => {
    if (element.contains(event.target)) return;
    if (open?.anchor?.contains(event.target)) return;
    closeMenu();
  };
  document.addEventListener("keydown", onKeydown, true);
  document.addEventListener("pointerdown", onOutside);
  return {
    element,
    anchor,
    dismiss() {
      document.removeEventListener("keydown", onKeydown, true);
      document.removeEventListener("pointerdown", onOutside);
      window.visualViewport?.removeEventListener("resize", position);
      window.visualViewport?.removeEventListener("scroll", position);
      window.removeEventListener("resize", position);
    },
  };
}

// Active interaction has to settle before its asynchronous persistence echo.
function persistMenu(value) {
  cachedMenuValue = value;
  if (menuRecord) void menuRecord.write(value);
}

function closeMenu({ restoreFocus = false, persist = true } = {}) {
  removeMenu(restoreFocus);
  pendingMenuFocus = false;
  if (persist) persistMenu({ open: false });
}

function removeMenu(restoreFocus) {
  if (!open) return;
  const anchor = open.anchor;
  open.anchor?.setAttribute("aria-expanded", "false");
  open.dismiss();
  open.element.remove();
  open = null;
  if (restoreFocus && anchor?.isConnected) anchor.focus();
}

function openJumpMenu(anchor, { list = MENU_FOR_SELECTOR[anchor.dataset.select] || "workspaces", query = "", focus = false } = {}) {
  open = {
    ...menuShell(anchor, "tbmenu"),
    select: anchor.dataset.select,
    mode: "jump",
    list,
    query,
  };
  anchor.setAttribute("aria-expanded", "true");
  paintMenuShell();
  const filter = open.element.querySelector(".tb-filter");
  if (focus) filter?.focus();
  paintMenu();
  if (focus && !filter) open.element.querySelector('[role="menuitem"]')?.focus();
}

function applyMenuRecord(saved) {
  cachedMenuValue = saved;
  if (!scopeReady) return;
  if (!saved?.open) {
    return paintClosedMenu();
  }
  const anchor = $("#toolbar")?.querySelector(`[data-select="${saved.select}"]`);
  if (!anchor) return;
  const list = menuList(saved);
  if (waitForRouteScope(list, saved)) return;
  paintOpenMenu(anchor, list, typeof saved.query === "string" ? saved.query : "", saved.select);
}

function paintClosedMenu() {
  removeMenu(pendingMenuFocus);
  pendingMenuFocus = false;
}

const menuList = (saved) => MENU_LISTS[saved.list] ? saved.list : (MENU_FOR_SELECTOR[saved.select] || "workspaces");

function waitForRouteScope(list, saved) {
  if (open || list !== "workspaces" || App.route.name !== "workspace") return false;
  const routeKey = routeProjectKey(App.route);
  if (!routeKey || scopedProjectKey === routeKey) return false;
  void rememberScope(routeKey).then(() => {
    if (cachedMenuValue === saved) applyMenuRecord(saved);
  });
  return true;
}

function paintOpenMenu(anchor, list, query, select) {
  if (!open || open.select !== select) {
    removeMenu(false);
    openJumpMenu(anchor, { list, query, focus: pendingMenuFocus });
  } else {
    open.list = list;
    open.query = query;
    paintMenu();
    const filter = open.element.querySelector(".tb-filter");
    if (filter && filter.value !== query) filter.value = query;
    if (pendingMenuFocus) focusMenuChoice(filter);
  }
  pendingMenuFocus = false;
}

function focusMenuChoice(filter) {
  (filter || open.element.querySelector('[aria-checked="true"]') || open.element.querySelector('[role="menuitem"]'))?.focus();
}

/** Move the open menu to the other list. The query goes with the list it was
 *  typed against — the two lists hold different kinds of name. */
function showList(list) {
  open.list = list;
  open.query = "";
  paintMenuShell();
  focusMenuChoice(open.element.querySelector(".tb-filter"));
  paintMenu();
  persistMenu({ open: true, select: open.select, list, query: "" });
}

/// Whichever list the open menu is showing. Repainted in place as the query
/// changes and as the feed moves, so a menu left open does not go stale.
///
/// The frame is built once for the list it is showing, and only the rows are
/// reconciled — matched by the project or work item each one names. So the box
/// being typed into is never replaced, the caret in it never moves, and a feed
/// tick under an open menu redraws only the rows that actually changed.
function paintMenu() {
  if (!open || open.mode !== "jump") return;
  paintMenuShell();
  const entries = (MENU_LISTS[open.list] || workspaceMenuEntries)();
  patchList(open.element.querySelector(".tbmenu-list"), entries, {
    keyOf: (entry) => entry.key,
    render: (entry) => entry.html,
  });
  updateMenuReturnLink();
  const reviewHost = open.element.querySelector(".tbmenu-review");
  if (reviewHost) {
    const review = workspaceReviewMenuEntry();
    patchList(reviewHost, review ? [review] : [], { keyOf: () => "review", render: (entry) => entry.html });
  }
}

// #541 can identify a parent from its cached children before project.list
// supplies that parent's name. Returns still use that device-qualified identity.
function returnProject() {
  const project = scopedProject();
  if (project) return project;
  const picker = toolbarDestinationModel(App.route, feed, { projectKey: scopedProjectKey });
  return picker.groups[0]?.destination.project || null;
}

function updateMenuReturnLink() {
  const menu = open;
  const link = menu.element.querySelector("[data-project-page]");
  const project = returnProject();
  if (!link || !project) return;
  const route = App.route;
  void projectReturnTarget(project, { route, currentHash: currentHash() }).then((target) => {
    if (open === menu && App.route === route && link.isConnected && scopedProjectKey === project.projectKey) link.href = target.hash;
  });
}

/** The workspace name menu offers review only for the place the route stands.
 * A different project selected inside this same popup owns no action on it. */
function workspaceForReviewMenu() {
  if (App.route.name !== "workspace" || open?.select !== "workspace") return null;
  if (scopedProject()?.projectKey !== routeProjectKey(App.route)) return null;
  return standingWorkspace();
}

function workspaceReviewMenuEntry() {
  const workspace = workspaceForReviewMenu();
  if (!workspace) return null;
  const support = reviewSupportFor(null, reviewSupport);
  const review = workspace.active_review;
  if (review?.task_id && review.workspace_id === workspace.id) {
    return support.pullRequests && support.get ? reviewMenuEntry("View pull request", "Open this workspace’s task review", review.task_id) : null;
  }
  return support.open ? reviewMenuEntry("Open review", "Publish this workspace as a pull request") : null;
}

const reviewMenuEntry = (label, description, taskId = null) => ({
  taskId,
  html: `<button class="mi" data-workspace-review type="button" role="menuitem"><span class="mt">${label}</span>
    <span class="md">${description}</span></button>`,
});

function openStandingReview() {
  const review = workspaceReviewMenuEntry();
  if (!review) return;
  const { deviceId, projectId, workspaceId } = App.route;
  closeMenu();
  if (review.taskId) {
    go({ name: "trackerTask", deviceId, projectId, taskId: review.taskId });
    return;
  }
  void import("./workspaceReviewEntry.js")
    .then(({ openWorkspaceReview }) => openWorkspaceReview({ deviceId, projectId, workspaceId, navigate: go }))
    .catch((error) => notifyError("Could not open review", error?.message || String(error)));
}

/** The frame the rows sit in: the filter, and — on the workspace list — the
 *  project scope and create action. Switching lists is a different menu, so that is the
 *  one thing that builds it again. */
function paintMenuShell() {
  if (open.element.dataset.list === open.list) return;
  open.element.dataset.list = open.list;
  open.element.innerHTML = (MENU_SHELLS[open.list] || workspaceMenuShellHtml)();
  open.element.onclick = onMenuClick;
  open.element.onkeydown = onMenuKeydown;
  const filter = open.element.querySelector(".tb-filter");
  if (filter) filter.oninput = () => {
    open.query = filter.value;
    paintMenu();
    persistMenu({ open: true, select: open.select, list: open.list, query: filter.value });
  };
}

/** The project half's pick: the menu moves to what is inside that project,
 *  which is its workspaces — read from that project's own machine. A legacy
 *  branch or task route stays readable in the bar's sentence, but it is not a
 *  place the menu offers to go any more. */
function pickProject(element) {
  if (!element) return false;
  const project = projectFor(element.dataset.project);
  void rememberScope(element.dataset.project);
  showList("workspaces");
  loadWorkspaces(project);
  return true;
}

function pickWorkspace(element) {
  if (!element) return false;
  const selected = scopedDestinations().find((candidate) => candidate.key === element.dataset.workspace);
  const route = App.route;
  closeMenu();
  if (!selected) return true;
  void navigationSelectionTarget(selected, { route, currentHash: currentHash() }).then((target) => {
    if (App.route === route && !target.current) go(target.route);
  });
  return true;
}

function cycleMenuTab(event) {
  const controls = [...open.element.querySelectorAll("*")]
    .filter((node) => node.matches('input, button:not(:disabled), a[href]'));
  const index = controls.indexOf(document.activeElement);
  const step = event.shiftKey ? -1 : 1;
  event.preventDefault();
  controls[(index + step + controls.length) % controls.length]?.focus();
}

function onMenuKeydown(event) {
  if (event.key === "Tab") return cycleMenuTab(event);
  const rows = [...open.element.querySelectorAll('.tbmenu-list [role="menuitem"]')];
  const index = rows.indexOf(document.activeElement);
  const moves = { ArrowDown: 1, ArrowUp: -1 };
  if (moves[event.key]) {
    event.preventDefault();
    const start = moves[event.key] > 0 ? 0 : rows.length - 1;
    const next = index < 0 ? start : (index + moves[event.key] + rows.length) % rows.length;
    rows[next]?.focus();
  } else if (event.key === "Enter" && document.activeElement.matches(".tb-filter")) {
    event.preventDefault();
    rows[0]?.click();
  }
}

function manageWorkspaces() {
  const project = returnProject();
  closeMenu();
  if (project) go({ name: "project", deviceId: project.deviceId, projectId: project.id, tab: "workspaces" });
}

const MENU_ACTIONS = [
  ["[data-menu-close]", () => closeMenu({ restoreFocus: true })],
  ["[data-manage-workspaces]", manageWorkspaces],
  ["[data-projects]", () => showList("projects")],
  ["[data-project-page]", () => openProjectPage()],
  ["[data-workspace-review]", openStandingReview],
  ["[data-create]", openCreate],
];

function onMenuClick(event) {
  const { target } = event;
  if (target.closest("a") && !plainActivation(event)) return;
  if (target.closest("a")) event.preventDefault();
  const action = MENU_ACTIONS.find(([selector]) => target.closest(selector));
  if (action) return action[1]();
  if (pickProject(target.closest("[data-project]"))) return;
  pickWorkspace(target.closest("[data-workspace]"));
}

/** The one create surface, on the scoped project and on the machine that
 *  project is on. An account with no project has nowhere to create, and says
 *  so. */
function openCreate() {
  const project = scopedProject();
  closeMenu();
  if (!project) {
    notifyError("No project to create in.", "Add a project in Settings first.");
    return;
  }
  openCreateWork({ projectId: project.id, deviceId: project.deviceId, projectName: nameOf(project), navigate: go });
}

/** The project half's rows: which project — said with its machine when another
 *  machine uses the same name — and how much is waiting in it. */
function projectMenuEntries() {
  const projects = projectMenuModel({
    projects: projectsOf(),
    items: feed.items,
    devices: App.devices,
    projectKey: scopedProject()?.projectKey,
    query: open.query,
  });
  if (!projects.length) return [{ key: "none", html: `<div class="tb-none dim">No project by that name.</div>` }];
  return projects.map((project) => ({
    key: `project:${project.key}`,
    html: `<button class="mi${project.current ? " current" : ""}" data-project="${esc(project.key)}" type="button" role="menuitem">
               <span class="mi-line"><span class="mt">${esc(project.name)}${deviceTagHtml(project)}</span>${unreadBadgeHtml(project.unreadCount, project.name)}</span></button>`,
  }));
}

/** The project half's frame. */
function projectMenuShellHtml() {
  return `${menuHeading("Switch to")}
    <input class="tb-filter mini" type="text" placeholder="Jump to a project" value="${esc(open.query)}"
      aria-label="Filter projects" ${fieldTraits("search", "go")} />
    <div class="tbmenu-list"></div>`;
}

/** The scoped project's workspaces, as the open menu lists them, and what a
 *  clicked row is looked up in. */
const scopedPicker = () => toolbarDestinationModel(App.route, feed, {
  devices: App.devices, projectKey: scopedProject()?.projectKey || routeProjectKey(App.route),
  currentHash: currentHash(), query: open.query,
});
const scopedDestinations = () => scopedPicker().groups.flatMap((group) => group.destinations);

function workspaceMenuEntries() {
  const picker = scopedPicker();
  const entries = picker.groups.flatMap((group) => group.destinations);
  const rows = entries.map((destination) => ({
    key: `${destination.kind}:${destination.key}`,
    html: `<a class="mi${destination.current ? " current" : ""}" ${destination.kind === "project" ? "data-project-page" : `data-workspace="${esc(destination.key)}"`}
      href="${esc(destination.hash)}" role="menuitem"${destination.current ? ' aria-current="page"' : ""}>
      <span class="mt">${esc(destination.kind === "project" ? "Project workspace" : destination.name)}</span>
      <span class="md">${esc(destination.current ? "Current" : destination.statusText || "Return to this project")}</span></a>`,
  }));
  if (!entries.some((entry) => entry.kind === "workspace")) {
    const empty = open.query ? "No workspace by that name." : picker.roster?.present ? "No workspaces in this project" : "No cached workspaces yet";
    rows.push({ key: "none", html: `<div class="tb-none dim">${empty}</div>` });
  }
  return rows;
}

const menuHeading = (title) => `<div class="tbmenu-head"><strong>${esc(title)}</strong>
  <button type="button" data-menu-close aria-label="Close switcher">Close</button></div>`;

/** What the scoped project is called, for the lines the menus head themselves
 *  with. */
const scopedName = () => nameOf(returnProject()) || "This project";

function workspaceMenuShellHtml() {
  return `${menuHeading(`Workspaces in ${scopedName()}`)}
    <input class="tb-filter mini" type="text" placeholder="Jump to a workspace" value="${esc(open.query)}"
      aria-label="Filter workspaces" ${fieldTraits("search", "go")} />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName())}</span>
      <button class="tb-scope-switch" data-projects type="button">All projects</button>
    </div>
    <div class="tbmenu-list"></div>
    <div class="tbmenu-foot">
      <button class="mi" data-manage-workspaces type="button" role="menuitem"><span class="mt">Manage workspaces</span></button>
      <div class="tb-group">Workspace actions</div>
      <div class="tbmenu-review"></div>
      <button class="mi" data-create="workspace" type="button" role="menuitem"><span class="mt">New workspace…</span>
        <span class="md">Materialize every source in ${esc(scopedName())}</span></button>
    </div>`;
}

/** Which list each selector opens. A selector is a kind, so it is a table
 *  rather than a chain; a trigger this table does not name opens the workspace
 *  list, which is the bar's own. */
const MENU_FOR_SELECTOR = {
  project: "projects",
  workspace: "workspaces",
};

/** Which rows each list of the jump menu offers, and the frame each sits in.
 *  A list is a kind, so it is a table rather than a chain. */
const MENU_LISTS = {
  projects: projectMenuEntries,
  workspaces: workspaceMenuEntries,
};

const MENU_SHELLS = {
  projects: projectMenuShellHtml,
  workspaces: workspaceMenuShellHtml,
};

// ---- mounting ----------------------------------------------------------------

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function initToolbar() {
  if (mounted) {
    paint({ entering: true });
    return toolbarReady;
  }
  mounted = true;
  const run = ++toolbarRun;
  scopedProjectKey = null;
  scopeReady = false;
  unsubscribeFeed = subscribeFeed((next) => {
    feed = next;
    shownFeed = filterByDevice(next, App.deviceFilter);
    // The menu is a view over the same records: a workspace made, renamed or
    // finished on another machine moves it on the delivery that carried it.
    loadStandingWorkspaces();
    paint();
  });
  scopeRecord = watchUiState(SCOPE_ADDRESS, (saved) => {
    // An active gesture owns its scope; an older/cross-tab echo cannot steal it.
    if (open) return;
    scopedProjectKey = typeof saved?.projectKey === "string" ? saved.projectKey : null;
    paint();
  });
  toolbarReady = scopeRecord.ready.then(async () => {
    if (!mounted || run !== toolbarRun) return;
    scopeReady = true;
    const routeKey = routeProjectKey(App.route);
    if (routeKey) await rememberScope(routeKey);
    if (!mounted || run !== toolbarRun) return;
    paint({ entering: true });
    loadStandingWorkspaces();
    menuRecord = watchUiState(MENU_ADDRESS, applyMenuRecord);
    return menuRecord.ready;
  });
  return toolbarReady;
}

/** Repaint for the route the shell just entered — the one paint that re-scopes,
 *  because it is the one that follows a move. */
export function toolbarRouteChanged() {
  if (!mounted) return;
  paint({ entering: true });
  if (cachedMenuValue?.open) applyMenuRecord(cachedMenuValue);
  loadStandingWorkspaces();
}

/** Teardown, for tests and for a gate that tears the session down. */
export function stopToolbar() {
  const settled = Promise.all([scopeRecord?.flush(), menuRecord?.flush()]);
  mounted = false;
  stopLockSupport?.();
  stopLockSupport = null;
  lockDeviceId = null;
  lockSupported = false;
  stopReviewSupport?.();
  stopReviewSupport = null;
  stopToolbarMeasurement?.();
  stopToolbarMeasurement = null;
  stopTaskLabel?.();
  stopTaskLabel = null;
  taskKey = null;
  taskLabel = null;
  returnRead += 1;
  reviewDeviceId = null;
  reviewSupport = NO_REVIEW_SUPPORT;
  toolbarRun += 1;
  scopeRecord?.dispose({ flushPending: false });
  menuRecord?.dispose({ flushPending: false });
  scopeRecord = null;
  menuRecord = null;
  scopeReady = false;
  cachedMenuValue = null;
  scopedProjectKey = null;
  pendingMenuFocus = false;
  unsubscribeFeed?.();
  unsubscribeFeed = null;
  workspacesByProject.clear();
  closeMenu({ persist: false });
  return settled;
}
