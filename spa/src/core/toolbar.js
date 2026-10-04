// The view area's toolbar: where you are standing, how to go somewhere else,
// and what the work you are standing in is doing.
//
// Left: on a workspace, one picker reading `project / workspace` and nothing
// else — the workspace's directories, its Tasks and its settings are its
// navigation, and stand in the rail down its left edge (core/directoryRail.js).
// The picker's popup starts with the active project's workspaces and the way to
// the project's own page; Switch project moves the same popup to the project
// list, and a project choice moves it back after loading that project's
// workspaces. Legacy branch and task links retain their project selector and
// static item identity while those routes remain supported.
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
import { projectMenuModel, toolbarIdentity, workspaceMenuModel } from "./toolbarModel.js";
import { deviceTagHtml, projectNameOf } from "./inboxProjects.js";
import { filterByDevice } from "./deviceFilter.js";
import { deviceKey, routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { deviceView } from "./feedMerge.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { patchList } from "./patchList.js";
import { toolbarHtml, unreadBadgeHtml } from "./toolbarRender.js";
import { workspaceRoute } from "./projectModel.js";
import { projectReturnRoute } from "./projectRailState.js";
import { standsOnProjectCheckout, workspaceStatusText } from "./workspaceModel.js";
import "../styles/shell.css";
import { fieldTraits } from "./fieldTraits.js";

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
  return projectFor(scopedProjectKey) || projectsOf()[0] || null;
}

function rememberScope(projectKey) {
  if (!projectKey || projectKey === scopedProjectKey) return Promise.resolve();
  return scopeRecord ? scopeRecord.write({ projectKey }) : Promise.resolve();
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
function loadWorkspaces(project) {
  if (!project?.projectKey) return;
  workspacesByProject.set(project.projectKey, workspaceRows(project.deviceId, project.id));
  paint();
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
// eslint-disable-next-line complexity -- ratchet: paint is at 13, cap 10 — reduce it, then drop this line
function paint({ entering = false } = {}) {
  const host = $("#toolbar");
  if (!host || !scopeReady) return;
  const standing = identity();
  // Navigating into a work item scopes the menu to its project — the toolbar
  // reads as one sentence, so the two halves can never name different projects.
  // A route that names a project but no work item scopes it too: that is where
  // a project link lands now, since a project's own checkout is not a surface
  // (core/router.js).
  if (entering) rememberScope(standing.projectKey || routeProjectKey(App.route));
  const shown = shownIdentity(standing);
  const signature = JSON.stringify(shown);
  // A poll tick that says the same thing the bar already shows must leave the
  // DOM alone: the verb slot (setToolbarVerb) can carry a view's own open menu
  // or in-flight action, and rebuilding out from under it would close the one
  // or double-fire the other. A navigation always rebuilds regardless — it is
  // the one paint that re-scopes the menu below.
  if (entering || signature !== paintedIdentity || !host.querySelector(".toolbar")) {
    paintedIdentity = signature;
    host.innerHTML = toolbarHtml(shown);
    // A repaint replaces the very buttons a menu hangs off, so an open menu is
    // re-pointed at the new one — otherwise its anchor is a detached node and
    // the selector that opened it stops toggling it shut.
    if (open) {
      open.anchor = host.querySelector(`[data-select="${open.select}"]`) || open.anchor;
      open.anchor.setAttribute("aria-expanded", "true");
    }
    host.querySelectorAll("[data-select]").forEach((control) => {
      control.onclick = (event) => {
        event.stopPropagation();
        if (open && open.anchor === control) {
          closeMenu();
          return;
        }
        const select = control.dataset.select;
        const list = MENU_FOR_SELECTOR[select] || "workspaces";
        pendingMenuFocus = true;
        if (menuRecord) void menuRecord.write({ open: true, select, list, query: "" });
      };
    });
  }
  paintVerb();
  if (open) paintMenu();
}

/** What the bar draws for where the route stands: the project (the scoped one
 *  where the route names none), the kind and the work item's label. */
const shownIdentity = (standing) => ({
  project: standing.project || nameOf(scopedProject()),
  kind: standing.kind,
  label: standing.label,
});

/** An explicit return restores the project's last face. Ordinary project
 * links still open Tasks. A later navigation supersedes a pending UI read. */
async function openProjectPage() {
  const project = scopedProject();
  const standing = App.route;
  closeMenu();
  if (!project) return;
  const route = await projectReturnRoute({ id: project.id, deviceId: project.deviceId });
  if (App.route === standing) go(route);
}

// ---- the menu each selector opens -------------------------------------------

function menuShell(anchor, className) {
  const element = document.createElement("div");
  element.className = className;
  element.setAttribute("role", "menu");
  // Fixed to the viewport under its anchor, like the tab row's menus: the
  // toolbar is a narrow row and would clip a child popup.
  const box = anchor.getBoundingClientRect ? anchor.getBoundingClientRect() : { left: 0, right: 0, bottom: 0 };
  element.style.position = "fixed";
  element.style.left = `${Math.max(8, Math.min(box.left, (window.innerWidth || 0) - 340))}px`;
  element.style.top = `${box.bottom + 4}px`;
  document.body.appendChild(element);

  const onKeydown = (event) => {
    if (event.key === "Escape") closeMenu({ restoreFocus: true });
  };
  const onOutside = (event) => {
    if (element.contains(event.target)) return;
    if (anchor.contains && anchor.contains(event.target)) return;
    closeMenu();
  };
  document.addEventListener("keydown", onKeydown);
  document.addEventListener("pointerdown", onOutside);
  return {
    element,
    anchor,
    dismiss() {
      document.removeEventListener("keydown", onKeydown);
      document.removeEventListener("pointerdown", onOutside);
    },
  };
}

function closeMenu({ restoreFocus = false, persist = true } = {}) {
  if (persist && menuRecord) {
    pendingMenuFocus = restoreFocus;
    void menuRecord.write({ open: false });
    return;
  }
  removeMenu(restoreFocus);
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
  paintMenu();
  const filter = open.element.querySelector(".tb-filter");
  const initialChoice = open.element.querySelector('[aria-checked="true"]') || open.element.querySelector("[role=menuitem]");
  if (focus) (filter || initialChoice)?.focus();
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
  pendingMenuFocus = true;
  if (menuRecord) void menuRecord.write({ open: true, select: open.select, list, query: "" });
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
}

/** The frame the rows sit in: the filter, and — on the workspace list — the
 *  project scope and create action. Switching lists is a different menu, so that is the
 *  one thing that builds it again. */
function paintMenuShell() {
  if (open.element.dataset.list === open.list) return;
  open.element.dataset.list = open.list;
  open.element.innerHTML = (MENU_SHELLS[open.list] || workspaceMenuShellHtml)();
  open.element.onclick = onMenuClick;
  const filter = open.element.querySelector(".tb-filter");
  if (filter) filter.oninput = () => {
    if (menuRecord) void menuRecord.write({ open: true, select: open.select, list: open.list, query: filter.value });
  };
}

/** The project half's pick: the menu moves to what is inside that project,
 *  which is its workspaces — read from that project's own machine. A legacy
 *  branch or task route stays readable in the bar's sentence, but it is not a
 *  place the menu offers to go any more. */
function pickProject(element) {
  if (!element) return false;
  const project = projectFor(element.dataset.project);
  void rememberScope(element.dataset.project).then(() => {
    if (!open) return;
    showList("workspaces");
    loadWorkspaces(project);
  });
  return true;
}

function pickWorkspace(element) {
  if (!element) return false;
  const selected = scopedWorkspaces().find((candidate) => candidate.workspaceKey === element.dataset.workspace);
  closeMenu();
  const route = workspaceRoute(selected);
  if (route) go(route);
  return true;
}

function onMenuClick(event) {
  const { target } = event;
  if (target.closest("[data-projects]")) return showList("projects");
  if (target.closest("[data-project-page]")) return openProjectPage();
  if (pickProject(target.closest("[data-project]"))) return;
  if (pickWorkspace(target.closest("[data-workspace]"))) return;
  const create = target.closest("[data-create]");
  if (create) openCreate();
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
  return `
    <input class="tb-filter mini" type="text" placeholder="Jump to a project" value="${esc(open.query)}"
      aria-label="Filter projects" ${fieldTraits("search", "go")} />
    <div class="tbmenu-list"></div>`;
}

/** The scoped project's workspaces, as the open menu lists them, and what a
 *  clicked row is looked up in. */
const scopedWorkspaces = () =>
  workspaceMenuModel({
    workspaces: workspacesByProject.get(scopedProject()?.projectKey) || [],
    projectKey: scopedProject()?.projectKey,
    workspaceKey: routeWorkspaceKey(App.route),
    query: open.query,
  });

function workspaceMenuEntries() {
  const entries = scopedWorkspaces();
  // Nothing is being waited for: the menu is a view over the records, and an
  // empty one is a project with no workspace by that name in them.
  if (!entries.length) return [{ key: "none", html: '<div class="tb-none dim">No workspace by that name.</div>' }];
  return entries.map((workspace) => ({
    key: `workspace:${workspace.workspaceKey}`,
    html: `<button class="mi${workspace.current ? " current" : ""}" data-workspace="${esc(workspace.workspaceKey)}" type="button" role="menuitem">
      <span class="mt">${esc(workspace.name)}</span>
      <span class="md">${esc(workspaceStatusText(workspace))}</span></button>`,
  }));
}

/** What the scoped project is called, for the lines the menus head themselves
 *  with. */
const scopedName = () => nameOf(scopedProject()) || "This project";

function workspaceMenuShellHtml() {
  return `
    <input class="tb-filter mini" type="text" placeholder="Jump to a workspace" value="${esc(open.query)}"
      aria-label="Filter workspaces" ${fieldTraits("search", "go")} />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName())}</span>
      <button class="tb-scope-switch" data-projects type="button">Switch project</button>
    </div>
    <div class="tbmenu-list"></div>
    <div class="tbmenu-foot">
      <button class="mi" data-project-page type="button" role="menuitem"><span class="mt">Project page</span>
        <span class="md">The tasks and workspaces of ${esc(scopedName())}</span></button>
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
