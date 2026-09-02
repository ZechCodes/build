// The view area's toolbar: where you are standing, how to go somewhere else,
// and what the work you are standing in is doing.
//
// Left: the project name, then the branch or issue name. They are two words of
// one sentence and each opens the menu of its own kind — the project name lists
// projects, the branch-or-issue name lists that project's branches and issues,
// with the two creates at its foot. Picking a project re-scopes and hands you
// straight to its work list (there is no project page any more; branches and
// issues are the only navigation targets); picking work goes there; picking a
// create asks for the one thing it needs and opens what it made.
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
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { notifyError } from "./notify.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import { agentChoiceParams, agentChoicePanelHtml, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { branchNamePreview, projectMenuModel, toolbarIdentity, workMenuModel } from "./toolbarModel.js";
import { patchList } from "./patchList.js";
import "../styles/shell.css";

const SCOPE_KEY = "build.toolbar.project";
/** Names the create form's three harness controls, so its panel and compose's
 *  can be open at once without either answering for the other. */
const CHOICE_PREFIX = "tb-choice";

let feed = { items: [], projects: [] };
let scopedProjectId = null;
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
function paint({ entering = false } = {}) {
  const host = $("#toolbar");
  if (!host) return;
  const standing = identity();
  // Navigating into a work item scopes the menu to its project — the toolbar
  // reads as one sentence, so the two halves can never name different projects.
  if (entering && standing.projectId) rememberScope(standing.projectId);
  const shown = {
    project: standing.project || projectNameOf(scopeProjectId()),
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
  // A create form is a question in flight: the feed may move under it, and its
  // answer is not repainted away.
  if (open && !(open.create && open.create.busy)) paintMenu();
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
    create: null,
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
  if (open.create) {
    paintCreate();
    return;
  }
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
    // There is no project page to go to: picking a project scopes the toolbar
    // and hands you its work, which is what you came for.
    rememberScope(project.dataset.project);
    showList("work");
    return;
  }
  const work = event.target.closest("[data-work]");
  if (work) {
    const entry = workMenuModel({ items: feed.items, projectId: scopeProjectId(), query: open.query }).find(
      (candidate) => candidate.key === work.dataset.work,
    );
    closeMenu();
    if (entry) go(entry.route);
    return;
  }
  const create = event.target.closest("[data-create]");
  if (create) {
    open.create = newCreate(create.dataset.create);
    paintMenu();
  }
}

/** A create form's state. The harness starts at the account's defaults — the
 *  panel is where a create says otherwise, and it starts shut. `navigate` is
 *  how the thing made is opened: the toolbar's own creates go straight there,
 *  the rail's put the rail away first on a narrow viewport. */
function newCreate(kind, navigate = go) {
  return { kind, busy: false, error: "", value: "", choice: loadAgentDefaults(), choiceOpen: false, navigate };
}

/** The create form, opened from somewhere other than the toolbar's own menu —
 *  the rail's project blocks. It is the same form in the same popup, scoped to
 *  the project named (which re-scopes the toolbar too: the two must never name
 *  different projects), and cancelling it shuts the popup whole, since there
 *  is no list behind it to come back to. */
export function openCreateFrom(anchor, { projectId, kind, navigate = go }) {
  closeMenu();
  rememberScope(projectId);
  open = {
    ...menuShell(anchor, "tbmenu"),
    select: "",
    mode: "jump",
    list: "work",
    query: "",
    standalone: true,
    create: newCreate(kind, navigate),
  };
  paintMenu();
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

/** The item half's rows: the scoped project's branches and issues. */
function workMenuEntries() {
  const work = workMenuModel({ items: feed.items, projectId: scopeProjectId(), query: open.query });
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
  const scopedName = projectNameOf(scopeProjectId()) || "This project";
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a branch or issue" value="${esc(open.query)}"
      aria-label="Filter branches and issues" autocomplete="off" />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName)}</span>
      <button class="tb-scope-switch" data-projects type="button">Projects</button>
    </div>
    <div class="tbmenu-list"></div>
    <div class="tbmenu-foot">
      <button class="mi" data-create="branch" type="button" role="menuitem"><span class="mt">New branch…</span>
        <span class="md">A checkout of its own, in ${esc(scopedName)}</span></button>
      <button class="mi" data-create="issue" type="button" role="menuitem"><span class="mt">New issue…</span>
        <span class="md">Nothing runs until your first message</span></button>
    </div>`;
}

// ---- the two creates --------------------------------------------------------

/** What each create asks for. One field, because one field is all it needs: a
 *  branch needs a name, an issue needs what you want. */
const CREATE_COPY = {
  branch: {
    title: "New branch",
    hint: "A checkout and a branch of its own. Nothing is dispatched — the first message you send starts an agent there.",
    placeholder: "e.g. mascot model spike",
    label: "Name",
  },
  issue: {
    title: "New issue",
    hint: "Say what you want. No planning agent starts until you send the first message.",
    placeholder: "e.g. Add a /health endpoint that returns build SHA and uptime…",
    label: "Goal",
  },
};

/** The harness question, on the create that can answer it.
 *
 *  An issue carries its agent's provider, model and effort from the moment it
 *  is filed. A new branch carries no agent at all — nothing runs there until an
 *  agent is added or a dispatch names one — so asking would be asking about
 *  something that does not exist yet. */
function createChoiceHtml() {
  if (open.create.kind !== "issue") return "";
  return agentChoicePanelHtml(App.modelCatalog || { providers: [] }, open.create.choice, {
    prefix: CHOICE_PREFIX,
    open: open.create.choiceOpen,
  });
}

function wireCreateChoice(host) {
  const holder = host.querySelector(".agent-choice");
  if (!holder) return;
  holder.querySelector("[data-agent-choice-toggle]").onclick = () => {
    open.create.choiceOpen = !open.create.choiceOpen;
    paintCreate();
  };
  const onChange = (changed) => () => {
    open.create.choice = reconcileAgentChoice(readAgentChoice(host, CHOICE_PREFIX), changed);
    paintCreate();
  };
  const control = (field) => holder.querySelector(`#${CHOICE_PREFIX}-${field}`);
  control("provider").onchange = onChange({ providerChanged: true });
  control("model").onchange = onChange({ modelChanged: true });
  control("effort").onchange = onChange({});
}

function paintCreate() {
  const { kind, busy, error, value } = open.create;
  // The create takes the whole popup, frame and all, so coming back from it
  // builds the list's frame again.
  open.element.dataset.list = "create";
  const copy = CREATE_COPY[kind];
  const scopedName = projectNameOf(scopeProjectId()) || "this project";
  // The typed answer is state, not something the DOM happens to be holding: a
  // refused create repaints this form, and it must repaint with what was typed —
  // caret included, since the feed can tick while the question is still open.
  const typing = open.element.querySelector("#tb-create-input");
  const caret = typing && document.activeElement === typing ? typing.selectionStart : value.length;
  open.element.innerHTML = `
    <div class="tb-create">
      <div class="tb-create-head">${esc(copy.title)} in ${esc(scopedName)}</div>
      <div class="tb-create-hint dim">${esc(copy.hint)}</div>
      <label class="tb-create-label" for="tb-create-input">${esc(copy.label)}</label>
      ${
        kind === "issue"
          ? `<textarea id="tb-create-input" rows="3" placeholder="${esc(copy.placeholder)}">${esc(value)}</textarea>`
          : `<input id="tb-create-input" type="text" class="path" placeholder="${esc(copy.placeholder)}" autocomplete="off" value="${esc(value)}" />`
      }
      ${createChoiceHtml()}
      <div class="tb-create-row">
        <span class="dim mono tb-create-preview" id="tb-create-preview"></span>
        <button class="btn mini" data-create-cancel type="button">Cancel</button>
        <button class="btn mini primary" data-create-go type="button"${busy ? " disabled" : ""}>${busy ? "creating…" : "Create"}</button>
      </div>
      <div class="warn tb-create-error"${error ? "" : " hidden"}>${esc(error)}</div>
    </div>`;
  wireCreateChoice(open.element);
  const input = open.element.querySelector("#tb-create-input");
  input.focus();
  input.setSelectionRange(caret, caret);
  const preview = open.element.querySelector("#tb-create-preview");
  const sync = () => {
    open.create.value = input.value;
    if (kind === "branch") preview.textContent = branchNamePreview(input.value);
  };
  input.oninput = sync;
  sync();
  input.onkeydown = (event) => {
    if (event.key === "Enter" && (kind === "branch" || event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submitCreate(input.value);
    }
  };
  open.element.querySelector("[data-create-cancel]").onclick = () => {
    if (open.standalone) {
      closeMenu();
      return;
    }
    open.create = null;
    paintMenu();
    open.element.querySelector(".tb-filter").focus();
  };
  open.element.querySelector("[data-create-go]").onclick = () => submitCreate(input.value);
}

/** The create's harness choice, as create params. An empty choice sends
 *  nothing and the daemon's own default stands. */
function agentParams() {
  return agentChoiceParams(App.modelCatalog || { providers: [] }, open.create.choice);
}

async function submitCreate(raw) {
  if (!open || !open.create || open.create.busy) return;
  const { kind, navigate } = open.create;
  open.create.value = String(raw || "");
  const value = open.create.value.trim();
  if (!value) {
    open.create.error = kind === "branch" ? "Name it first." : "Describe the issue first.";
    paintCreate();
    return;
  }
  const projectId = scopeProjectId();
  if (!projectId) {
    open.create.error = "No project to create in.";
    paintCreate();
    return;
  }
  open.create.busy = true;
  open.create.error = "";
  paintCreate();
  try {
    const route = kind === "branch" ? await createBranch(projectId, value) : await createIssue(projectId, value);
    closeMenu();
    refreshFeed();
    // A freshly cut branch has nobody in it yet — the rail opens on the ghost
    // composer, and that is exactly where typing the first message belongs.
    if (kind === "branch") App.focusComposerOnMount = true;
    navigate(route);
  } catch (error) {
    if (!open || !open.create) {
      notifyError(kind === "branch" ? "Couldn't create the branch" : "Couldn't file the issue", error.message);
      return;
    }
    open.create.busy = false;
    open.create.error = error.message;
    paintCreate();
  }
}

async function createBranch(projectId, name) {
  const created = await App.call("worktree.create", { project_id: projectId, name });
  return { name: "branch", projectId: created.project_id || projectId, branch: created.branch, tab: "changes" };
}

/** Inert by contract: the record exists, and nothing runs behind it until the
 *  first message (the bridge dispatches planning on that post). */
async function createIssue(projectId, goal) {
  const created = await App.call("issue.create", { goal, project_id: projectId, dispatch: false, ...agentParams() });
  return { name: "issue", projectId: created.project_id || projectId, id: created.issue_id || created.plan_id };
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
      const projectId = scopeProjectId();
      closeMenu();
      if (action === "archive") go({ name: "account", page: "archive" });
      else if (projectId) openProjectSettings(projectId);
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
    scopedProjectId = localStorage.getItem(SCOPE_KEY) || null;
  } catch {
    scopedProjectId = null;
  }
  subscribeFeed((next) => {
    feed = { items: next.items || [], projects: next.projects || [] };
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
