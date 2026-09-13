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
import { subscribeFeed } from "./taskFeed.js";
import {
  projectMenuModel,
  toolbarIdentity,
  workspaceDirectoryModel,
  workspaceMenuModel,
} from "./toolbarModel.js";
import { patchList } from "./patchList.js";
import { workspaceRoute } from "./projectModel.js";
import { openCreateWork } from "./createWork.js";
import "../styles/shell.css";

const SCOPE_KEY = "build.toolbar.project";

let feed = { items: [], projects: [] };
const workspacesByProject = new Map();
let workspaceRequest = 0;
let loadingWorkspaceProjectId = null;
let scopedProjectId = null;
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

function paintVerb() {
  const host = $("#tb-verb");
  if (!host) return;
  if (verbRender) verbRender(host);
  else host.innerHTML = "";
}

const projectsOf = () => feed.projects || [];
const projectNameOf = (projectId) => {
  const project = projectsOf().find((entry) => entry.id === projectId);
  return project ? project.name || project.id : "";
};

/** The project the menu is scoped to: where you are standing, else the last
 *  place you stood, else the first project the device knows. */
function scopeProjectId() {
  if (scopedProjectId && projectsOf().some((project) => project.id === scopedProjectId)) return scopedProjectId;
  const first = projectsOf()[0];
  return first ? first.id : scopedProjectId;
}

function rememberScope(projectId) {
  if (!projectId || projectId === scopedProjectId) return;
  scopedProjectId = projectId;
  try {
    localStorage.setItem(SCOPE_KEY, projectId);
  } catch {
    /* private mode: the scope just lasts the session */
  }
}

const normalizeWorkspace = (workspace) => ({
  ...(workspace || {}),
  id: workspace?.workspace_id || workspace?.id,
});

const workspaceAnswer = (answer) => normalizeWorkspace(answer?.workspace || answer);

async function workspaceRows(projectId, selectedWorkspaceId) {
  const listed = await App.call("workspace.list", { project_id: projectId });
  const rows = (listed?.workspaces || []).map(normalizeWorkspace);
  if (!selectedWorkspaceId) return rows;
  const selected = rows.find((workspace) => workspace.id === selectedWorkspaceId);
  if (Array.isArray(selected?.directories)) return rows;
  const detail = workspaceAnswer(await App.call("workspace.get", { workspace_id: selectedWorkspaceId }));
  return detail.id ? [...rows.filter((workspace) => workspace.id !== detail.id), detail] : rows;
}

function acceptWorkspaceRows(request, projectId, rows) {
  if (request !== workspaceRequest) return;
  workspacesByProject.set(projectId, rows);
  loadingWorkspaceProjectId = null;
  paint();
}

/** Read the workspace menu for one project, then hydrate the selected row so
 * its directory tabs are available even when `workspace.list` is summarized.
 * A route or project switch overtakes an older answer rather than letting it
 * repaint another project's toolbar. */
async function loadWorkspaces(projectId, selectedWorkspaceId = null) {
  if (!projectId || !App.call) return;
  const request = ++workspaceRequest;
  loadingWorkspaceProjectId = projectId;
  paint();
  try {
    acceptWorkspaceRows(request, projectId, await workspaceRows(projectId, selectedWorkspaceId));
  } catch {
    acceptWorkspaceRows(request, projectId, []);
  }
}

// ---- the bar ----------------------------------------------------------------

/** Pure: the toolbar's markup for one identity. Names come from repos, agents
 *  and the user, so every one of them is escaped. */
export function toolbarHtml({ project, kind, label, directories = [] }) {
  const identity = kind === "workspace"
    ? `<button class="tb-sel tb-workspace" data-select="workspace" type="button" aria-haspopup="menu" aria-expanded="false">
         <span class="tb-name">${esc(label)}</span><span class="tb-caret">▾</span>
       </button>`
    : kind
      ? `<button class="tb-sel tb-project" data-select="project" type="button" aria-haspopup="menu" aria-expanded="false">
           <span class="tb-name">${esc(project || "Projects")}</span><span class="tb-caret">▾</span>
         </button>
         <span class="tb-sep">/</span><span class="tb-legacy-item"><span class="tb-name${kind === "branch" ? " mono" : ""}">${esc(label)}</span></span>`
      : `<button class="tb-sel tb-project" data-select="project" type="button" aria-haspopup="menu" aria-expanded="false">
           <span class="tb-name">${esc(project || "Projects")}</span><span class="tb-caret">▾</span>
         </button>`;
  const directoryTabs = directories.length
    ? `<div class="tb-directories" role="tablist" aria-label="Workspace directories">${directories
        .map(
          (directory) =>
            `<button class="tb-directory${directory.current ? " current" : ""}" data-directory="${esc(directory.sourceId)}" type="button" role="tab" aria-selected="${directory.current ? "true" : "false"}">${esc(directory.label)}</button>`,
        )
        .join("")}</div>
       <button class="tb-sel tb-directory-menu" data-select="directory" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="Choose workspace directory">
         <span class="tb-name">${esc(directories.find((directory) => directory.current)?.label || directories[0].label)}</span><span class="tb-caret">▾</span>
       </button>`
    : "";
  return `<div class="toolbar">
    ${identity}
    ${directoryTabs}
    <div class="tb-right"><span class="tb-verb" id="tb-verb"></span></div>
  </div>`;
}

function identity() {
  return toolbarIdentity(App.route, {
    ...feed,
    workspaces: workspacesByProject.get(App.route.projectId) || [],
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
  // Navigating into a workspace scopes its popup to that workspace's project.
  if (entering && standing.projectId) rememberScope(standing.projectId);
  const shown = {
    project: standing.project || projectNameOf(scopeProjectId()),
    kind: standing.kind,
    label: standing.label,
    directories: standing.directories || [],
  };
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
        closeMenu();
        openJumpMenu(control);
      };
    });
    host.querySelectorAll("[data-directory]").forEach((control) => {
      control.onclick = () => openWorkspaceDirectory(control.dataset.directory);
    });
  }
  paintVerb();
  if (open) paintMenu();
}

function openWorkspaceDirectory(sourceId) {
  if (App.route.name !== "workspace" || !sourceId) return;
  const workspace = (workspacesByProject.get(App.route.projectId) || []).find((candidate) => candidate.id === App.route.workspaceId);
  const directory = workspaceDirectoryModel(workspace, sourceId).find((candidate) => candidate.sourceId === sourceId);
  if (!directory) return;
  go({
    name: "workspace",
    projectId: App.route.projectId,
    workspaceId: App.route.workspaceId,
    sourceId,
    tab: directory.is_git === false ? "files" : "changes",
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
  const list = anchor.dataset.select === "project"
    ? "projects"
    : anchor.dataset.select === "directory"
      ? "directories"
      : "workspaces";
  if (list === "workspaces" && App.route.name === "workspace") rememberScope(App.route.projectId);
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
  const entries = open.list === "projects"
    ? projectMenuEntries()
    : open.list === "directories"
      ? directoryMenuEntries()
      : workspaceMenuEntries();
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
  open.element.innerHTML = open.list === "projects"
    ? projectMenuShellHtml()
    : open.list === "directories"
      ? directoryMenuShellHtml()
      : workspaceMenuShellHtml();
  open.element.onclick = onMenuClick;
  const filter = open.element.querySelector(".tb-filter");
  if (filter) filter.oninput = () => {
    open.query = filter.value;
    paintMenu();
  };
}

/** Every row the menu offers, answered in one place — a row that has just
 *  arrived is live without having been wired. */
function pickProject(element) {
  if (!element) return false;
  const projectId = element.dataset.project;
  rememberScope(projectId);
  showList("workspaces");
  const selectedWorkspaceId = App.route.name === "workspace" && App.route.projectId === projectId
    ? App.route.workspaceId
    : null;
  void loadWorkspaces(projectId, selectedWorkspaceId);
  return true;
}

function pickWorkspace(element) {
  if (!element) return false;
  const selected = workspaceMenuModel({
    workspaces: workspacesByProject.get(scopeProjectId()) || [],
    projectId: scopeProjectId(),
    workspaceId: App.route.workspaceId,
    query: open.query,
  }).find((candidate) => candidate.id === element.dataset.workspace);
  closeMenu();
  const route = workspaceRoute(selected);
  if (route) go(route);
  return true;
}

function onMenuClick(event) {
  const target = event.target;
  if (target.closest("[data-projects]")) return showList("projects");
  const directory = target.closest("[data-menu-directory]");
  if (directory) {
    closeMenu();
    openWorkspaceDirectory(directory.dataset.menuDirectory);
    return;
  }
  if (pickProject(target.closest("[data-project]"))) return;
  if (pickWorkspace(target.closest("[data-workspace]"))) return;
  if (target.closest('[data-create="workspace"]')) {
    const projectId = scopeProjectId();
    closeMenu();
    openCreateWork({ projectId, projectName: projectNameOf(projectId) });
  }
}

function directoryMenuEntries() {
  if (App.route.name !== "workspace") return [];
  const workspace = (workspacesByProject.get(App.route.projectId) || []).find((candidate) => candidate.id === App.route.workspaceId);
  return workspaceDirectoryModel(workspace, App.route.sourceId).map((directory) => ({
    key: `directory:${directory.sourceId}`,
    html: `<button class="mi${directory.current ? " current" : ""}" data-menu-directory="${esc(directory.sourceId)}" type="button" role="menuitemradio" aria-checked="${directory.current ? "true" : "false"}">
      <span class="mt">${esc(directory.label)}</span></button>`,
  }));
}

function directoryMenuShellHtml() {
  return `<div class="tbmenu-list" aria-label="Workspace directories"></div>`;
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}

/** The project half's rows: which project, and how much is waiting in it. */
function projectMenuEntries() {
  const projects = projectMenuModel({
    projects: projectsOf(),
    items: feed.items,
    projectId: scopeProjectId(),
    query: open.query,
  });
  if (!projects.length) return [{ key: "none", html: `<div class="tb-none dim">No project by that name.</div>` }];
  return projects.map((project) => ({
    key: `project:${project.id}`,
    html: `<button class="mi${project.current ? " current" : ""}" data-project="${esc(project.id)}" type="button" role="menuitem">
               <span class="mi-line"><span class="mt">${esc(project.name)}</span>${unreadBadgeHtml(project.unreadCount, project.name)}</span></button>`,
  }));
}

/** The project half's frame. */
function projectMenuShellHtml() {
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a project" value="${esc(open.query)}"
      aria-label="Filter projects" autocomplete="off" />
    <div class="tbmenu-list"></div>`;
}

function workspaceMenuEntries() {
  const projectId = scopeProjectId();
  const entries = workspaceMenuModel({
    workspaces: workspacesByProject.get(projectId) || [],
    projectId,
    workspaceId: App.route.workspaceId,
    query: open.query,
  });
  if (!entries.length) {
    const message = loadingWorkspaceProjectId === projectId ? "Loading workspaces…" : "No workspace by that name.";
    return [{ key: "none", html: `<div class="tb-none dim">${message}</div>` }];
  }
  return entries.map((workspace) => ({
    key: `workspace:${workspace.id}`,
    html: `<button class="mi${workspace.current ? " current" : ""}" data-workspace="${esc(workspace.id)}" type="button" role="menuitem">
      <span class="mt">${esc(workspace.name)}</span>
      <span class="md">${esc(workspace.status || "")}</span></button>`,
  }));
}

function workspaceMenuShellHtml() {
  const scopedName = projectNameOf(scopeProjectId()) || "This project";
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a workspace" value="${esc(open.query)}"
      aria-label="Filter workspaces" autocomplete="off" />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName)}</span>
      <button class="tb-scope-switch" data-projects type="button">Switch project</button>
    </div>
    <div class="tbmenu-list"></div>
    <div class="tbmenu-foot">
      <button class="mi" data-create="workspace" type="button" role="menuitem"><span class="mt">New workspace…</span>
        <span class="md">Materialize every source in ${esc(scopedName)}</span></button>
    </div>`;
}

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
    scopedProjectId = localStorage.getItem(SCOPE_KEY) || null;
  } catch {
    scopedProjectId = null;
  }
  subscribeFeed((next) => {
    feed = { items: next.items || [], projects: next.projects || [] };
    paint();
  });
  paint({ entering: true });
  observeToolbar($("#toolbar"));
  if (App.route.name === "workspace") void loadWorkspaces(App.route.projectId, App.route.workspaceId);
}

/** Repaint for the route the shell just entered — the one paint that re-scopes,
 *  because it is the one that follows a move. */
export function toolbarRouteChanged() {
  if (!mounted) return;
  closeMenu();
  paint({ entering: true });
  if (App.route.name === "workspace") void loadWorkspaces(App.route.projectId, App.route.workspaceId);
}

/** Teardown, for tests and for a gate that tears the session down. */
export function stopToolbar() {
  workspaceRequest += 1;
  toolbarResizeObserver?.disconnect();
  toolbarResizeObserver = null;
  closeMenu();
}
