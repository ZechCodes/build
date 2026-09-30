// The project's own surface: what the project holds, and who you talk to about
// it.
//
// A project is a template — its checkout is the base every workspace is cut
// FROM, and nothing opens it — so this page is about the project itself. The
// main pane is the project's workspaces, each row opening its own surface; the
// rail is the project's agent, whose conversation the bridge keeps in a scratch
// directory of its own (planning/v2/workspaces.md).
//
// The page's verb sits in the toolbar's verb slot beside the project's name:
// the + that makes the first workspace, named for the project the bar names.
//
// Two tabs, because a project holds two kinds of thing: the workspaces the work
// happens in, and the tasks that say what the work IS. They are the faces of
// the project's rail (core/projectRail.js, #274), with the project's Settings at
// its foot, the way a workspace's rail carries its own. The Tasks tab is its
// own surface (core/trackerTasksPane.js) mounted into this page's body, and it
// keeps its own URL — `#/project/<p>/tasks` — so a link to a board opens one.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { esc } from "../core/text.js";
import { collapseChatOverPage, shellSelection } from "../core/shell.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { clearToolbarVerb, setToolbarVerb } from "../core/toolbar.js";
import { openCreateWork } from "../core/createWork.js";
import { mountProjectRail } from "../core/projectRail.js";
import {
  ALL_WORKSPACES,
  RECLAIMABLE_WORKSPACES,
  projectPageModel,
  workspaceListing,
} from "../core/projectPageModel.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import { ICON_PLUS } from "../core/icons.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { mountTasksPane } from "../core/trackerTasksPane.js";
import { hashFromRoute } from "../core/router.js";
import "../styles/tasks.css";
import "../styles/surfaces.css";

/** The two tabs, and which one a route stands on. Workspaces is the page
 *  itself, so a route that names no tab names that one. */
const WORKSPACES_TAB = "workspaces";
const TASKS_TAB = "tasks";
/// Tasks is the default, so a route that names no tab is on it (#46).
const tabOf = (route) => (route.tab === WORKSPACES_TAB ? WORKSPACES_TAB : TASKS_TAB);

/** Line two of a workspace row: what it is standing on, what its checkout is
 *  doing when that is not simply "ready", and what the work weighs. The same
 *  three facts the rail's rows carry, with the room this page has for them. */
const factsLine = (row) =>
  [row.branch, row.status === "ready" ? "" : row.statusText, row.facts].filter(Boolean).join(" · ");

const unreadHtml = (row) => (row.unreadCount > 0 ? `<span class="badge inbox-unread">${row.unreadCount}</span>` : "");

/** Line three, when the reclaim service has something to say (#135): what holds
 *  a quiet workspace, or that nothing does. The bridge refusing a Reclaim says
 *  why in its own words, under it. */
const lifecycleHtml = (row, ui) => {
  if (!row.lifecycle) return "";
  const error = ui.reclaimErrors.get(row.workspaceKey) || "";
  return `<div class="inbox-facts project-lifecycle">${esc(row.lifecycle.text)}</div>
      <span class="warn" data-reclaim-error${error ? "" : " hidden"}>${esc(error)}</span>`;
};

/** Reclaim, where nothing holds the workspace. It removes the workspace, whose
 *  work is already pushed and whose tasks are finished, so it asks nothing
 *  first: the bridge measures again and refuses if that stopped being true. */
const reclaimHtml = (row, ui) => {
  if (!row.lifecycle?.reclaimable) return "";
  const pending = ui.reclaiming.has(row.workspaceKey);
  return `<div class="inbox-actions"><button class="btn mini" type="button" data-workspace-reclaim="${esc(row.workspaceKey)}" aria-label="Reclaim workspace ${esc(row.name)}"${pending ? " disabled" : ""}>${pending ? "Reclaiming…" : "Reclaim"}</button></div>`;
};

/** One workspace, in the rail's own row shape: a project page and the rail are
 *  two views of the same list, so they read as the same list. */
const rowHtml = (row, ui) => `<div class="srow inbox-entry project-row${row.muted ? " inbox-muted" : ""}" data-workspace="${esc(row.workspaceKey)}">
    <span class="sdot sdot-${esc(row.state)}" title="${esc(row.state)}"></span>
    <div class="inbox-body">
      <div class="inbox-line inbox-name"><span class="stitle">${esc(row.name)}</span>${unreadHtml(row)}</div>
      <div class="inbox-facts">${esc(factsLine(row))}</div>
      ${lifecycleHtml(row, ui)}
    </div>
    <span class="project-size">${esc(row.sizeText)}</span>
    ${reclaimHtml(row, ui)}
  </div>`;

/** A project nobody has cut a workspace in yet. That is the first state of
 *  every project, not a broken one, so the page says what to do about it and
 *  points at the control that does it. */
const emptyHtml = () => `<div class="empty project-empty">
    <h2>No workspaces yet</h2>
    <p>A workspace is where the work happens in this project. The + above makes the first one.</p>
  </div>`;

/** One of the two filters (#167), as first stood up; `paintFilters` says
 *  which is chosen and how many can be reclaimed. */
const filterButtonHtml = (filter, label) =>
  `<button class="btn mini project-filter" type="button" data-workspace-filter="${filter}" aria-pressed="false">${esc(label)}</button>`;

const filtersHtml = () => `<div class="project-filters" role="group" aria-label="Which workspaces to show">
    ${filterButtonHtml(ALL_WORKSPACES, "All")}
    ${filterButtonHtml(RECLAIMABLE_WORKSPACES, "Reclaimable")}
  </div>`;

const listingHtml = (listing, ui) =>
  listing.empty
    ? `<p class="project-filter-empty">No workspace can be reclaimed right now.</p>`
    : `<div class="project-rows">${listing.rows.map((row) => rowHtml(row, ui)).join("")}</div>`;

/** The filters stay the nodes they were across repaints: a press or a feed
 *  move must not take the focused one away (review 1). Only what they say
 *  changes. The reclaimable one says how many there are, so the press is not a
 *  guess. */
function paintFilters(pane, listing) {
  for (const button of pane.querySelectorAll("[data-workspace-filter]")) {
    const active = button.dataset.workspaceFilter === listing.filter;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", String(active));
    if (button.dataset.workspaceFilter === RECLAIMABLE_WORKSPACES) {
      button.textContent = `Reclaimable (${listing.reclaimableCount})`;
    }
  }
}

/** The rows are rebuilt; a Reclaim that had focus gets it back afterwards,
 *  found by the workspace it reclaims. */
function paintListing(host, listing, ui) {
  const focused = host.contains(document.activeElement)
    ? document.activeElement.closest("[data-workspace-reclaim]")?.dataset.workspaceReclaim
    : null;
  host.innerHTML = listingHtml(listing, ui);
  if (!focused) return;
  [...host.querySelectorAll("[data-workspace-reclaim]")]
    .find((button) => button.dataset.workspaceReclaim === focused)
    ?.focus();
}

/** The Workspaces tab: the empty state, or the filters over the rows. */
function paintWorkspaces(pane, ui) {
  if (ui.page.empty) {
    pane.innerHTML = emptyHtml();
    return;
  }
  if (!pane.querySelector("[data-project-listing]")) {
    pane.innerHTML = `${filtersHtml()}<div data-project-listing></div>`;
  }
  const listing = workspaceListing(ui.page, ui.filter);
  paintFilters(pane, listing);
  paintListing(pane.querySelector("[data-project-listing]"), listing, ui);
}

/** The verb this page owns, in the toolbar's slot. Called on every toolbar
 *  repaint, so it rebuilds only when the project it names has changed. */
function paintProjectVerbs(host, state) {
  const name = state.page.name;
  if (host.dataset.project === name && host.querySelector("[data-project-create]")) return;
  host.dataset.project = name;
  host.innerHTML = `<button class="iconbtn" type="button" data-project-create aria-label="New workspace in ${esc(name)}" title="New workspace in ${esc(name)}">${ICON_PLUS}</button>`;
  host.querySelector("[data-project-create]").onclick = () => createWorkspace(state);
}

/** The + : one workspace in this project, on the machine the project is on. */
function createWorkspace(state) {
  openCreateWork({
    projectId: state.route.projectId,
    deviceId: state.context.deviceId,
    projectName: state.page.name,
    navigate: go,
  });
}

function paint(state) {
  setToolbarVerb(state.verb);
  const pane = $("#project-pane");
  if (!pane) return;
  if (state.tab === TASKS_TAB) {
    state.tasks?.feedMoved();
    return;
  }
  const shown = JSON.stringify([state.page.rows, state.filter, [...state.reclaiming], [...state.reclaimErrors]]);
  if (pane.dataset.rows !== shown) {
    pane.dataset.rows = shown;
    paintWorkspaces(pane, state);
  }
}

/**
 * Open one of the two tabs.
 *
 * Each owns the body outright — the workspaces list paints into it and the
 * Tasks pane mounts into it — so the one leaving is torn down before the one
 * arriving is built. The URL is rewritten rather than navigated: the page is
 * the same page, and a navigation would remount the rail beside it.
 */
function openTab(state, tab) {
  if (state.tab === tab) return;
  state.tasks?.dispose();
  state.tasks = null;
  state.tab = tab;
  state.rail.paint(tab);
  const pane = $("#project-pane");
  pane.innerHTML = "";
  delete pane.dataset.rows;
  if (tab === TASKS_TAB) mountTasks(state, pane);
  else paint(state);
  writeTabHash(state);
}

/** The hash this page is standing at, kept in step with the tab and the view
 *  without a navigation. A reload lands back on what is on screen. */
function writeTabHash(state) {
  // The tab is always named on the route; it is the URL that leaves the default
  // out (core/router.js `projectTabPath`).
  const route = { ...state.route, tab: state.tab, view: state.view };
  App.route = route;
  state.route = route;
  history.replaceState(null, "", hashFromRoute(route));
}

function mountTasks(state, pane) {
  state.tasks = mountTasksPane(pane, {
    projectId: state.route.projectId,
    projectName: state.page.name,
    deviceId: state.context.deviceId,
    projectKey: routeProjectKey(state.route),
    callRpc: state.context.rpc,
    catalog: () => state.context.modelCatalog(),
    refreshCatalog: () => state.context.refreshModelCatalog(),
    feed: () => state.feed,
    navigate: go,
    view: state.view,
    onViewChange: (view) => {
      state.view = view;
      writeTabHash(state);
    },
  });
}

/** Reclaim one workspace (#135). The row repaints from the feed once the
 *  bridge has removed it; a refusal stays on the row until the next press. */
async function reclaimWorkspace(state, workspaceKey) {
  const row = state.page.rows.find((candidate) => candidate.workspaceKey === workspaceKey);
  if (!row || state.reclaiming.has(workspaceKey)) return;
  state.reclaiming.add(workspaceKey);
  state.reclaimErrors.delete(workspaceKey);
  paint(state);
  try {
    await state.context.rpc("workspace.reclaim", { workspace_id: row.workspaceId });
  } catch (error) {
    state.reclaimErrors.set(workspaceKey, error?.message || String(error));
  } finally {
    state.reclaiming.delete(workspaceKey);
    if (!state.disposed) paint(state);
  }
  await refreshFeed(state.context.deviceId);
}

/** A press in the list: a filter narrows it, Reclaim reclaims, anywhere else
 *  on a row opens that workspace. */
function pressList(state, event) {
  const filter = event.target.closest("[data-workspace-filter]");
  if (filter) {
    state.filter = filter.dataset.workspaceFilter;
    paint(state);
    return;
  }
  const reclaim = event.target.closest("[data-workspace-reclaim]");
  if (reclaim) {
    reclaimWorkspace(state, reclaim.dataset.workspaceReclaim);
    return;
  }
  const row = event.target.closest("[data-workspace]");
  if (row) openWorkspace(state, row.dataset.workspace);
}

/** A row opens its workspace. The row carries the route the rail would have
 *  opened it with, so the same workspace opens the same way from either list. */
function openWorkspace(state, workspaceKey) {
  const row = state.page.rows.find((candidate) => candidate.workspaceKey === workspaceKey);
  if (row?.route) go(row.route);
}

export async function renderProject() {
  const root = $("#root");
  const route = App.route;
  const context = surfaceContext(route);
  root.className = "surface";
  // The surface paints what the records hold of this machine whether or not it
  // can answer. Only a machine nothing here has ever held has nothing to paint:
  // the notice names it, waits for it, and hands the link back when it lands.
  if (!context) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const state = {
    route, context, disposed: false, selection: shellSelection(),
    page: projectPageModel(null, route), verb: null,
    tab: tabOf(route), view: route.view || "dashboard", feed: null, tasks: null, rail: null,
    reclaiming: new Set(), reclaimErrors: new Map(), filter: ALL_WORKSPACES,
  };
  state.verb = (host) => paintProjectVerbs(host, state);
  root.innerHTML = `<div id="tabbody" class="flush"><div id="project-pane" class="project-page"></div></div>`;
  // The two tabs are the rail's faces. A press switches in place rather than
  // navigating, because a navigation would remount the agent rail beside the
  // page; the shell's route rule never sees it (#62), so where the chat lies
  // over the page the press puts it away itself.
  state.rail = mountProjectRail($("#dir-rail"), {
    route,
    context,
    navigate: go,
    onSelect: (tab) => {
      collapseChatOverPage();
      openTab(state, tab === WORKSPACES_TAB ? WORKSPACES_TAB : TASKS_TAB);
    },
  });
  state.rail.paint(state.tab);
  $("#project-pane").onclick = (event) => pressList(state, event);
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => !state.page.empty });
  const unsubscribe = subscribeFeed((feed) => {
    if (state.disposed) return;
    state.feed = feed;
    state.page = projectPageModel(feed, state.route);
    paint(state);
  });
  if (state.tab === TASKS_TAB) mountTasks(state, $("#project-pane"));
  paint(state);
  App.viewDispose = () => {
    state.disposed = true;
    unsubscribe();
    deviceStrip();
    state.tasks?.dispose();
    clearToolbarVerb(state.verb);
    state.rail.dispose();
  };
}
