// The view area's toolbar: where you are standing, how to go somewhere else,
// and what the work you are standing in is doing.
//
// Left: the project name, then the branch or issue name. They are two words of
// one sentence and each opens the menu of its own kind — the project name lists
// projects, the branch-or-issue name lists that project's branches and issues,
// with the two creates at its foot. Picking a project re-scopes and hands you
// straight to its work list (there is no project page any more; branches and
// issues are the only navigation targets); picking work goes there; picking a
// create opens the one create surface (core/createWork.js) on that project.
//
// Right: a slot the standing view can fill with its own verb — a branch's
// Done, say — then the ⋯ that carries what used to be the tab row's right
// cluster (the archive and the project's settings). The working-time ticker
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
import { openProjectSettings } from "../sheets/projectSettings.js";
import { openCreateWork } from "./createWork.js";
import { projectMenuModel, toolbarIdentity, workMenuModel } from "./toolbarModel.js";
import { deviceTagHtml, projectNameOf } from "./inboxProjects.js";
import { patchList } from "./patchList.js";
import { projectRoute } from "./projectModel.js";
import "../styles/shell.css";

const SCOPE_KEY = "build.toolbar.project";

let feed = { items: [], projects: [] };
// The account-wide name of the scoped project (core/deviceKey.js), never the
// bare id: every machine mints a `proj-1`, and the menu lists them all.
let scopedProjectKey = null;
let open = null; // { element, anchor, mode, dismiss } while the menu is up
let mounted = false;
let paintedIdentity = null; // what the bar's OWN markup was last built from — see paint()
let verbRender = null; // (host) => void, the standing view's own verb-slot paint

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

/** Every machine's projects: the toolbar names one project on one machine, and
 *  it offers all of them to move to. */
const projectsOf = () => feed.projects || [];
const projectFor = (projectKey) => projectsOf().find((project) => project.projectKey === projectKey) || null;
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

// ---- the bar ----------------------------------------------------------------

/** Pure: the toolbar's markup for one identity. Names come from repos, agents
 *  and the user, so every one of them is escaped. */
export function toolbarHtml({ project, kind, label }) {
  const itemSelector = kind
    ? `<span class="tb-sep">/</span>
       <button class="tb-sel tb-item" data-select="item" type="button" aria-haspopup="menu">
         <span class="tb-name${kind === "branch" ? " mono" : ""}">${esc(label)}</span><span class="tb-caret">▾</span>
       </button>`
    : "";
  return `<div class="toolbar">
    <button class="tb-sel tb-project" data-select="project" type="button" aria-haspopup="menu">
      <span class="tb-name">${esc(project || "Projects")}</span><span class="tb-caret">▾</span>
    </button>
    ${itemSelector}
    <div class="tb-right">
      <span class="tb-verb" id="tb-verb"></span>
      <button class="iconbtn tb-more" data-select="more" type="button" title="More" aria-label="More actions" aria-haspopup="menu">⋯</button>
    </div>
  </div>`;
}

function identity() {
  return toolbarIdentity(App.route, feed);
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
  if (entering && standing.projectKey) rememberScope(standing.projectKey);
  const shown = {
    project: standing.project || nameOf(scopedProject()),
    kind: standing.kind,
    label: standing.label,
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
    if (open) open.anchor = host.querySelector(`[data-select="${open.select}"]`) || open.anchor;
    host.querySelectorAll("[data-select]").forEach((control) => {
      control.onclick = (event) => {
        event.stopPropagation();
        const wanted = control.dataset.select;
        if (open && open.anchor === control) {
          closeMenu();
          return;
        }
        closeMenu();
        if (wanted === "more") openSurfaceMenu(control);
        else openJumpMenu(control);
      };
    });
  }
  paintVerb();
  if (open) paintMenu();
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
    if (event.key === "Escape") closeMenu();
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

function closeMenu() {
  if (!open) return;
  open.dismiss();
  open.element.remove();
  open = null;
}

function openJumpMenu(anchor) {
  open = {
    ...menuShell(anchor, "tbmenu"),
    select: anchor.dataset.select,
    mode: "jump",
    list: anchor.dataset.select === "project" ? "projects" : "work",
    query: "",
  };
  paintMenu();
  const filter = open.element.querySelector(".tb-filter");
  if (filter) filter.focus();
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
  const entries = open.list === "projects" ? projectMenuEntries() : workMenuEntries();
  patchList(open.element.querySelector(".tbmenu-list"), entries, {
    keyOf: (entry) => entry.key,
    render: (entry) => entry.html,
  });
}

/** The frame the rows sit in: the filter, and — on the work list — the scope
 *  line and the two creates. Switching lists is a different menu, so that is the
 *  one thing that builds it again. */
function paintMenuShell() {
  if (open.element.dataset.list === open.list) return;
  open.element.dataset.list = open.list;
  open.element.innerHTML = open.list === "projects" ? projectMenuShellHtml() : workMenuShellHtml();
  open.element.onclick = onMenuClick;
  const filter = open.element.querySelector(".tb-filter");
  filter.oninput = () => {
    open.query = filter.value;
    paintMenu();
  };
}

/** Every row the menu offers, answered in one place — a row that has just
 *  arrived is live without having been wired. */
function onMenuClick(event) {
  if (event.target.closest("[data-projects]")) {
    showList("projects");
    return;
  }
  const project = event.target.closest("[data-project]");
  if (project) {
    const selected = projectFor(project.dataset.project);
    rememberScope(project.dataset.project);
    if (selected?.is_git === false) {
      closeMenu();
      go(projectRoute(selected));
      return;
    }
    showList("work");
    return;
  }
  const work = event.target.closest("[data-work]");
  if (work) {
    const entry = workMenuModel({ items: feed.items, projectKey: scopedProject()?.projectKey, query: open.query }).find(
      (candidate) => candidate.key === work.dataset.work,
    );
    closeMenu();
    if (entry) go(entry.route);
    return;
  }
  const create = event.target.closest("[data-create]");
  if (create) openCreate(create.dataset.create);
}

/** The one create surface, on the scoped project and on the machine that
 *  project is on. An account with no project has nowhere to create, and says
 *  so. */
function openCreate(kind) {
  const project = scopedProject();
  closeMenu();
  if (!project) {
    notifyError("No project to create in.", "Add a project in Settings first.");
    return;
  }
  openCreateWork({ projectId: project.id, deviceId: project.deviceId, projectName: nameOf(project), kind });
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
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

/** The item half's rows: the scoped project's branches and issues. */
function workMenuEntries() {
  const work = workMenuModel({ items: feed.items, projectKey: scopedProject()?.projectKey, query: open.query });
  if (!work.length) return [{ key: "none", html: `<div class="tb-none dim">Nothing here yet.</div>` }];
  return work.map((entry) => ({
    key: `work:${entry.key}`,
    html: `<button class="mi" data-work="${esc(entry.key)}" type="button" role="menuitem">
               <span class="mi-line"><span class="mt${entry.kind === "branch" ? " mono" : ""}">${esc(entry.label)}</span>${unreadBadgeHtml(
                 entry.unreadCount,
                 entry.label,
               )}</span>
               <span class="md">${esc(entry.detail)}</span></button>`,
  }));
}

/** The item half's frame: the filter, the scope line above the rows, and the
 *  two creates at its foot. */
function workMenuShellHtml() {
  const project = scopedProject();
  const scopedName = nameOf(project) || "This project";
  const create = project && project.is_git === false
    ? ""
    : `<div class="tbmenu-foot">
      <button class="mi" data-create="branch" type="button" role="menuitem"><span class="mt">New branch…</span>
        <span class="md">A checkout of its own, in ${esc(scopedName)}</span></button>
      <button class="mi" data-create="issue" type="button" role="menuitem"><span class="mt">New issue…</span>
        <span class="md">Nothing runs until your first message</span></button>
    </div>`;
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a branch or issue" value="${esc(open.query)}"
      aria-label="Filter branches and issues" autocomplete="off" />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName)}</span>
      <button class="tb-scope-switch" data-projects type="button">Projects</button>
    </div>
    <div class="tbmenu-list"></div>
    ${create}`;
}

// ---- the ⋯ -------------------------------------------------------------------

/** What the tab row's right cluster used to carry. Archive is an account page
 *  now (the inbox is global, so an archive of it is too); settings is the
 *  project's own sheet. */
function openSurfaceMenu(anchor) {
  open = { ...menuShell(anchor, "tbmenu tbmenu-actions"), select: anchor.dataset.select, mode: "more" };
  open.element.innerHTML = `
    <button class="mi" data-action="archive" type="button" role="menuitem"><span class="mt">Archive</span>
      <span class="md">Work that has been finished</span></button>
    <button class="mi" data-action="settings" type="button" role="menuitem"><span class="mt">Project settings</span>
      <span class="md">Name, path, base branch, remote</span></button>`;
  open.element.querySelectorAll("[data-action]").forEach((row) => {
    row.onclick = () => {
      const action = row.dataset.action;
      const project = scopedProject();
      closeMenu();
      if (action === "archive") go({ name: "account", page: "archive" });
      else if (project) openProjectSettings(project.id);
    };
  });
}

// ---- mounting ----------------------------------------------------------------

/** Mount once. Re-entrant: a reconnect calls this again and it just repaints. */
export function initToolbar() {
  if (mounted) {
    paint({ entering: true });
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
    paint();
  });
  paint({ entering: true });
}

/** Repaint for the route the shell just entered — the one paint that re-scopes,
 *  because it is the one that follows a move. */
export function toolbarRouteChanged() {
  if (!mounted) return;
  closeMenu();
  paint({ entering: true });
}

/** Teardown, for tests and for a gate that tears the session down. */
export function stopToolbar() {
  closeMenu();
}
