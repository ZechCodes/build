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
// Right: the working-time ticker and the diffstat off the same feed row the
// inbox reads, and the ⋯ that carries what used to be the tab row's right
// cluster — the archive and the project's settings.
//
// The toolbar outlives views (it is the shell's row, not a view's), so it mounts
// once and repaints from the feed and the route.

import { $ } from "../dom.js";
import { esc } from "./text.js";
import { App, go } from "../app.js";
import { refreshFeed, subscribeFeed } from "./taskFeed.js";
import { notifyError } from "./notify.js";
import { loadAgentDefaults } from "./agentDefaults.js";
import { agentChoiceParams, agentChoicePanelHtml, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { branchNamePreview, projectMenuModel, toolbarIdentity, toolbarStatus, workMenuModel } from "./toolbarModel.js";
import "../styles/shell.css";

const SCOPE_KEY = "build.toolbar.project";
/** Names the create form's three harness controls, so its panel and compose's
 *  can be open at once without either answering for the other. */
const CHOICE_PREFIX = "tb-choice";

let feed = { items: [], projects: [] };
let scopedProjectId = null;
let open = null; // { element, anchor, mode, dismiss } while the menu is up
let ticker = null;
let mounted = false;

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
      <span class="tb-status" id="tb-status"></span>
      <button class="iconbtn tb-more" data-select="more" type="button" title="More" aria-label="More actions" aria-haspopup="menu">⋯</button>
    </div>
  </div>`;
}

/** Pure: the right side's two facts, each one only when it is known. */
export function statusHtml(row, nowMs = Date.now()) {
  const { working, stat } = toolbarStatus(row, nowMs);
  return (
    (working ? `<span class="tb-working">${esc(working)}</span>` : "") +
    (stat ? `<span class="tb-stat mono">${esc(stat)}</span>` : "")
  );
}

function identity() {
  return toolbarIdentity(App.route, feed);
}

function paintStatus() {
  const slot = $("#tb-status");
  if (slot) slot.innerHTML = statusHtml(identity().row);
}

function paint() {
  const host = $("#toolbar");
  if (!host) return;
  const standing = identity();
  // Standing in a work item scopes the menu to its project — the toolbar reads
  // as one sentence, so the two halves can never name different projects.
  if (standing.projectId) rememberScope(standing.projectId);
  host.innerHTML = toolbarHtml({
    project: standing.project || projectNameOf(scopeProjectId()),
    kind: standing.kind,
    label: standing.label,
  });
  // A repaint replaces the very buttons a menu hangs off, so an open menu is
  // re-pointed at the new one — otherwise its anchor is a detached node and the
  // selector that opened it stops toggling it shut.
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
  paintStatus();
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

/** Whichever list the open menu is showing. Repainted in place as the query
 *  changes and as the feed moves, so a menu left open does not go stale. */
function paintMenu() {
  if (!open || open.mode !== "jump") return;
  if (open.create) {
    paintCreate();
    return;
  }
  // A repaint must not steal the caret from whoever is typing in the filter —
  // the feed ticks under an open menu, and a search box that keeps jumping to
  // the end mid-word is worse than a stale list.
  const typing = open.element.querySelector(".tb-filter");
  const caret = typing && document.activeElement === typing ? typing.selectionStart : null;
  open.element.innerHTML = open.list === "projects" ? projectListHtml() : workListHtml();
  const filter = open.element.querySelector(".tb-filter");
  if (caret !== null) {
    filter.focus();
    filter.setSelectionRange(caret, caret);
  }
  filter.oninput = () => {
    open.query = filter.value;
    paintMenu();
  };
  open.element.querySelectorAll("[data-project]").forEach((row) => {
    row.onclick = () => {
      // There is no project page to go to: picking a project scopes the toolbar
      // and hands you its work, which is what you came for.
      rememberScope(row.dataset.project);
      showList("work");
    };
  });
  const back = open.element.querySelector("[data-projects]");
  if (back) back.onclick = () => showList("projects");
  open.element.querySelectorAll("[data-work]").forEach((row) => {
    row.onclick = () => {
      const entry = workMenuModel({ items: feed.items, projectId: scopeProjectId(), query: open.query }).find(
        (candidate) => candidate.key === row.dataset.work,
      );
      closeMenu();
      if (entry) go(entry.route);
    };
  });
  open.element.querySelectorAll("[data-create]").forEach((row) => {
    row.onclick = () => {
      // The harness starts at the account's defaults — the panel is where a
      // create says otherwise, and it starts shut.
      open.create = {
        kind: row.dataset.create,
        busy: false,
        error: "",
        value: "",
        choice: loadAgentDefaults(),
        choiceOpen: false,
      };
      paintMenu();
    };
  });
}

/** The counter a menu row wears: what is waiting inside it, and nothing at all
 *  when nothing is. Same badge the inbox rows use. */
function unreadBadgeHtml(count, what) {
  if (!count) return "";
  return `<span class="badge" title="${count} unread in ${esc(what)}">${count}</span>`;
}

/** The project half's list: which project, and how much is waiting in it. */
function projectListHtml() {
  const projects = projectMenuModel({
    projects: projectsOf(),
    items: feed.items,
    projectId: scopeProjectId(),
    query: open.query,
  });
  const rows = projects.length
    ? projects
        .map(
          (project) =>
            `<button class="mi${project.current ? " current" : ""}" data-project="${esc(project.id)}" type="button" role="menuitem">
               <span class="mi-line"><span class="mt">${esc(project.name)}</span>${unreadBadgeHtml(project.unreadCount, project.name)}</span></button>`,
        )
        .join("")
    : `<div class="tb-none dim">No project by that name.</div>`;
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a project" value="${esc(open.query)}"
      aria-label="Filter projects" autocomplete="off" />
    <div class="tbmenu-list">${rows}</div>`;
}

/** The item half's list: the scoped project's branches and issues, the two
 *  creates at its foot, and the way back to the projects. */
function workListHtml() {
  const scopedName = projectNameOf(scopeProjectId()) || "This project";
  const work = workMenuModel({ items: feed.items, projectId: scopeProjectId(), query: open.query });
  const rows = work.length
    ? work
        .map(
          (entry) =>
            `<button class="mi" data-work="${esc(entry.key)}" type="button" role="menuitem">
               <span class="mi-line"><span class="mt${entry.kind === "branch" ? " mono" : ""}">${esc(entry.label)}</span>${unreadBadgeHtml(
                 entry.unreadCount,
                 entry.label,
               )}</span>
               <span class="md">${esc(entry.detail)}</span></button>`,
        )
        .join("")
    : `<div class="tb-none dim">Nothing here yet.</div>`;
  return `
    <input class="tb-filter" type="text" placeholder="Jump to a branch or issue" value="${esc(open.query)}"
      aria-label="Filter branches and issues" autocomplete="off" />
    <div class="tb-group tb-scope">
      <span>${esc(scopedName)}</span>
      <button class="tb-scope-switch" data-projects type="button">Projects</button>
    </div>
    <div class="tbmenu-list">${rows}</div>
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
  const { kind } = open.create;
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
    go(route);
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
    paint();
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
  // The working time is a clock, not a poll: it ticks between feeds.
  ticker = setInterval(paintStatus, 1000);
  paint();
}

/** Repaint for the route the shell just entered. */
export function toolbarRouteChanged() {
  if (!mounted) return;
  closeMenu();
  paint();
}

/** Teardown, for tests and for a gate that tears the session down. */
export function stopToolbar() {
  if (ticker) clearInterval(ticker);
  ticker = null;
  closeMenu();
}
