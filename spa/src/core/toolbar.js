// The view area's toolbar: where you are standing, how to go somewhere else,
// and what the work you are standing in is doing.
//
// Left: a workspace switcher followed immediately by that workspace's directory
// tabs. Its popup starts with the active project's workspaces; Switch project
// moves the same popup to the project list, and a project choice moves it back
// after loading that project's workspaces. Legacy branch and issue links retain
// their project selector and static item identity while those routes remain supported.
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
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { workspaceAgents } from "./trackerAssignee.js";
import { WORKSPACE_ISSUES_SELECTOR, mountWorkspaceIssues } from "./trackerWorkspaceIssuesView.js";
import { workspaceIssuesPlace } from "./workspaceIssuesTab.js";
import { notifyError } from "./notify.js";
import { openCreateWork } from "./createWork.js";
import { openWorkspaceSettings } from "../sheets/workspaceSettings.js";
import { deviceCatalog } from "./inboxDevices.js";
import {
  projectMenuModel,
  toolbarIdentity,
  workspaceDirectoryModel,
  workspaceMenuModel,
} from "./toolbarModel.js";
import { deviceTagHtml, projectNameOf } from "./inboxProjects.js";
import { canAnswer, contextFor } from "./deviceContexts.js";
import { filterByDevice } from "./deviceFilter.js";
import { deviceKey, routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { deviceView } from "./feedMerge.js";
import { patchList } from "./patchList.js";
import { toolbarHtml, unreadBadgeHtml } from "./toolbarRender.js";
import { projectRoute, workspaceRoute } from "./projectModel.js";
import { directoryTab, standsOnProjectCheckout, workspaceStatusText } from "./workspaceModel.js";
import "../styles/shell.css";

const SCOPE_KEY = "build.toolbar.project";

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
/** The issues icon's block, while the bar is standing in a workspace. */
let issuesBlock = null;
let open = null; // { element, anchor, mode, dismiss } while the menu is up
let mounted = false;
let paintedIdentity = null; // what the bar's OWN markup was last built from — see paint()
let verbRender = null; // (host) => void, the standing view's own verb-slot paint
let toolbarResizeObserver = null;

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

// ---- the project's tabs ------------------------------------------------------

/** The project page standing under the bar, where there is one: a press on
 *  Workspaces or Issues is handed to it, so the page switches its tab in place
 *  rather than being torn down and built again around the same rail. From any
 *  other route — an issue's page — a press is a navigation to that tab. */
let projectTabHandler = null;

export function setProjectTabHandler(handler) {
  projectTabHandler = handler;
}

/** Only the page that set it can clear it (see clearToolbarVerb). */
export function clearProjectTabHandler(handler) {
  if (projectTabHandler === handler) projectTabHandler = null;
}

export function pressProjectTab(tab) {
  if (App.route.name === "project" && projectTabHandler) {
    projectTabHandler(tab);
    paint();
    return;
  }
  const { deviceId, projectId } = App.route;
  go({ ...projectRoute({ id: projectId, deviceId }), ...(tab === "issues" ? { tab: "issues" } : {}) });
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
  if (!projectKey || projectKey === scopedProjectKey) return;
  scopedProjectKey = projectKey;
  try {
    localStorage.setItem(SCOPE_KEY, projectKey);
  } catch {
    /* private mode: the scope just lasts the session */
  }
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

/** The workspace the route is standing in, off the rows its project's machine
 *  last listed. */
const standingWorkspace = () =>
  (workspacesByProject.get(routeProjectKey(App.route)) || []).find(
    (candidate) => candidate.workspaceKey === routeWorkspaceKey(App.route),
  ) || null;

/** Its directories, marked with the one the route is on. */
const standingDirectories = () =>
  App.route.name === "workspace" ? workspaceDirectoryModel(standingWorkspace(), App.route.sourceId) : [];

// ---- the bar ----------------------------------------------------------------

/** The agents of the workspace the bar is standing in, in the order its row
 *  lists them — the same order the rail's bubbles read across. Read at every
 *  paint rather than captured: a workspace gains and loses agents while the bar
 *  stands there. */
function agentsInFocus() {
  const route = App.route;
  if (route.name !== "workspace" || !route.workspaceId) return [];
  const group = workspaceAgents(feed, routeProjectKey(route))
    .find((candidate) => candidate.workspaceId === route.workspaceId);
  return (group?.agents || []).map((agent, index) => ({ id: agent.id, ordinal: index + 1 }));
}

/**
 * The issues icon beside the cog, kept in step with the bar.
 *
 * Mounted against the button the last repaint drew — a repaint replaces that
 * element, so the block is remounted onto the new one rather than left holding
 * a detached node. Where the bar drew no button (every identity but a
 * workspace) whatever was mounted is disposed.
 *
 * The count itself is not painted from here: the block listens to the project's
 * cached issue list and moves its own badge, so an `issues` push never has to
 * repaint the bar — the verb slot beside it can be holding a view's open menu
 * or an action in flight.
 */
function syncIssuesButton(host) {
  const button = host.querySelector(WORKSPACE_ISSUES_SELECTOR);
  if (!button) {
    issuesBlock?.dispose();
    issuesBlock = null;
    return;
  }
  if (issuesBlock?.button === button) {
    issuesBlock.refresh();
    return;
  }
  issuesBlock?.dispose();
  const block = mountWorkspaceIssues(button, {
    deviceId: App.route.deviceId,
    projectId: App.route.projectId,
    workspaceId: App.route.workspaceId,
    agents: agentsInFocus,
    // The icon is a shortcut to the workspace's Issues tab (#29), not a place
    // of its own: the overlay it used to open was a modal you had to close
    // before you could do anything about what was in it.
    open: () => go(workspaceIssuesPlace(App.route)),
  });
  issuesBlock = { ...block, button };
}

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
  if (!host) return;
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
    syncIssuesButton(host);
    host.querySelectorAll("[data-select]").forEach((control) => {
      control.onclick = (event) => {
        event.stopPropagation();
        if (open && open.anchor === control) {
          closeMenu();
          return;
        }
        closeMenu();
        openJumpMenu(control);
      };
    });
    host.querySelectorAll("[data-directory]").forEach((control) => {
      control.onclick = () => openWorkspaceDirectory(control.dataset.directory);
    });
    host.querySelectorAll("[data-project-tab]").forEach((control) => {
      control.onclick = () => pressProjectTab(control.dataset.projectTab);
    });
    const settings = host.querySelector("[data-workspace-settings]");
    if (settings) settings.onclick = () => openStandingWorkspaceSettings();
    const back = host.querySelector("[data-project-back]");
    if (back) back.onclick = () => goBackToProject();
  }
  paintVerb();
  if (open) paintMenu();
}

/** What the bar draws for where the route stands: the project (the scoped one
 *  where the route names none), the kind, the work item's label, and the tabs
 *  after it — a workspace's directories, or a project's two pages. */
const shownIdentity = (standing) => ({
  project: standing.project || nameOf(scopedProject()),
  kind: standing.kind,
  label: standing.label,
  directories: standing.directories || [],
  projectTabs: standing.projectTabs || [],
});

/** Out of the workspace, back to the project it was cut from — the project's
 *  own page, on the machine the workspace is on: the same place the project's
 *  name in the inbox opens (core/projectModel.js mints both). */
function goBackToProject() {
  const { deviceId, projectId } = App.route;
  return go(projectRoute({ id: projectId, deviceId }));
}

function openWorkspaceDirectory(sourceId) {
  const directory = sourceId ? standingDirectories().find((candidate) => candidate.sourceId === sourceId) : null;
  if (!directory) return;
  go({
    name: "workspace",
    deviceId: App.route.deviceId,
    projectId: App.route.projectId,
    workspaceId: App.route.workspaceId,
    sourceId,
    tab: directoryTab(directory),
  });
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

function closeMenu({ restoreFocus = false } = {}) {
  if (!open) return;
  const anchor = open.anchor;
  open.anchor?.setAttribute("aria-expanded", "false");
  open.dismiss();
  open.element.remove();
  open = null;
  if (restoreFocus && anchor?.isConnected) anchor.focus();
}

/** A directory popup can be open while the toolbar crosses its container
 * breakpoint. Dismiss it when its trigger becomes hidden so focus and menu
 * state never remain attached to an unavailable control. */
function reconcileOpenMenu() {
  if (open?.select !== "directory") return;
  if (getComputedStyle(open.anchor).display === "none") closeMenu();
}

function observeToolbar(host) {
  toolbarResizeObserver?.disconnect();
  if (typeof ResizeObserver !== "function") return;
  toolbarResizeObserver = new ResizeObserver(reconcileOpenMenu);
  toolbarResizeObserver.observe(host);
}

function openJumpMenu(anchor) {
  const list = MENU_FOR_SELECTOR[anchor.dataset.select] || "workspaces";
  if (list === "workspaces" && App.route.name === "workspace") rememberScope(routeProjectKey(App.route));
  open = {
    ...menuShell(anchor, "tbmenu"),
    select: anchor.dataset.select,
    mode: "jump",
    list,
    query: "",
  };
  anchor.setAttribute("aria-expanded", "true");
  paintMenu();
  const filter = open.element.querySelector(".tb-filter");
  const initialChoice = open.element.querySelector('[aria-checked="true"]') || open.element.querySelector("[role=menuitem]");
  (filter || initialChoice)?.focus();
}

/** Move the open menu to the other list. The query goes with the list it was
 *  typed against — the two lists hold different kinds of name. */
function showList(list) {
  open.list = list;
  open.query = "";
  paintMenu();
  open.element.querySelector(".tb-filter").focus();
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
    open.query = filter.value;
    paintMenu();
  };
}

/** The project half's pick: the menu moves to what is inside that project,
 *  which is its workspaces — read from that project's own machine. A legacy
 *  branch or issue route stays readable in the bar's sentence, but it is not a
 *  place the menu offers to go any more. */
function pickProject(element) {
  if (!element) return false;
  const project = projectFor(element.dataset.project);
  rememberScope(element.dataset.project);
  showList("workspaces");
  loadWorkspaces(project);
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
  const directory = target.closest("[data-menu-directory]");
  if (directory) {
    closeMenu();
    openWorkspaceDirectory(directory.dataset.menuDirectory);
    return;
  }
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

/** The cog's sheet, on the workspace the route is standing in and the machine
 *  that workspace is on.
 *
 *  The row the switcher last listed carries the name; a route whose list has
 *  not answered yet still has the name the bar is printing, so the sheet opens
 *  either way rather than making the reader wait for a read they can already
 *  see the result of. */
function openStandingWorkspaceSettings() {
  const { deviceId, workspaceId } = App.route;
  const context = contextFor(deviceId);
  if (!canAnswer(context)) {
    notifyError("That machine is not reachable.", "Workspace settings are read and written on the device the workspace is on.");
    return;
  }
  openWorkspaceSettings(
    {
      id: workspaceId,
      name: standingWorkspace()?.name || identity().label,
      workspaceKey: routeWorkspaceKey(App.route),
    },
    {
      callRpc: context.rpc,
      catalog: deviceCatalog(deviceId),
      // The name is printed by this bar and by every inbox row, so both are
      // told rather than left to their next poll.
      onRenamed: async () => {
        await refreshFeed(deviceId);
        loadWorkspaces(routeProject());
      },
      // Standing in a workspace that no longer exists is standing nowhere.
      onDeleted: async () => {
        workspacesByProject.delete(routeProjectKey(App.route));
        go({ name: "inbox" });
        await refreshFeed(deviceId);
      },
    },
  );
}

/** The directories of the workspace the route is standing in, as menu rows for
 *  the collapsed toolbar. */
function directoryMenuEntries() {
  return standingDirectories().map((directory) => ({
    key: `directory:${directory.sourceId}`,
    html: `<button class="mi${directory.current ? " current" : ""}" data-menu-directory="${esc(directory.sourceId)}" type="button" role="menuitemradio" aria-checked="${directory.current ? "true" : "false"}">
      <span class="mt">${esc(directory.label)}</span></button>`,
  }));
}

function directoryMenuShellHtml() {
  return `<div class="tbmenu-list" aria-label="Workspace directories"></div>`;
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
    <input class="tb-filter" type="text" placeholder="Jump to a project" value="${esc(open.query)}"
      aria-label="Filter projects" autocomplete="off" />
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
    <input class="tb-filter" type="text" placeholder="Jump to a workspace" value="${esc(open.query)}"
      aria-label="Filter workspaces" autocomplete="off" />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName())}</span>
      <button class="tb-scope-switch" data-projects type="button">Switch project</button>
    </div>
    <div class="tbmenu-list"></div>
    <div class="tbmenu-foot">
      <button class="mi" data-create="workspace" type="button" role="menuitem"><span class="mt">New workspace…</span>
        <span class="md">Materialize every source in ${esc(scopedName())}</span></button>
    </div>`;
}

/** Which list each selector opens. A selector is a kind, so it is a table
 *  rather than a chain; a trigger this table does not name opens the workspace
 *  list, which is the bar's own. */
const MENU_FOR_SELECTOR = {
  project: "projects",
  directory: "directories",
  workspace: "workspaces",
};

/** Which rows each list of the jump menu offers, and the frame each sits in.
 *  A list is a kind, so it is a table rather than a chain. */
const MENU_LISTS = {
  projects: projectMenuEntries,
  directories: directoryMenuEntries,
  workspaces: workspaceMenuEntries,
};

const MENU_SHELLS = {
  projects: projectMenuShellHtml,
  directories: directoryMenuShellHtml,
  workspaces: workspaceMenuShellHtml,
};

// ---- mounting ----------------------------------------------------------------

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function initToolbar() {
  if (mounted) {
    paint({ entering: true });
    observeToolbar($("#toolbar"));
    return;
  }
  mounted = true;
  try {
    scopedProjectKey = localStorage.getItem(SCOPE_KEY) || null;
  } catch {
    scopedProjectKey = null;
  }
  subscribeFeed((next) => {
    feed = next;
    shownFeed = filterByDevice(next, App.deviceFilter);
    // The menu is a view over the same records: a workspace made, renamed or
    // finished on another machine moves it on the delivery that carried it.
    loadStandingWorkspaces();
    paint();
  });
  paint({ entering: true });
  observeToolbar($("#toolbar"));
  loadStandingWorkspaces();
}

/** Repaint for the route the shell just entered — the one paint that re-scopes,
 *  because it is the one that follows a move. */
export function toolbarRouteChanged() {
  if (!mounted) return;
  closeMenu();
  paint({ entering: true });
  loadStandingWorkspaces();
}

/** Teardown, for tests and for a gate that tears the session down. */
export function stopToolbar() {
  workspacesByProject.clear();
  toolbarResizeObserver?.disconnect();
  toolbarResizeObserver = null;
  issuesBlock?.dispose();
  issuesBlock = null;
  closeMenu();
}
