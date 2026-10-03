// The project's own surface: what the project holds, and who you talk to about
// it.
//
// Tasks, Files and Workspaces share the project's rail. Files mounts the same
// explorer as the workspace surface, over the source directories workspaces
// are cut from. Each face rewrites its URL without remounting the agent rail.
// The toolbar's + creates a workspace; Settings remains at the rail's foot.

import { $ } from "../dom.js";
import { App, go, markRoute } from "../app.js";
import { esc } from "../core/text.js";
import { collapseChatOverPage, shellSelection } from "../core/shell.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { clearToolbarVerb, setToolbarVerb } from "../core/toolbar.js";
import { openCreateWork } from "../core/createWork.js";
import { renderFilesTab } from "./files.js";
import { rememberProjectRailRoute } from "../core/projectRailState.js";
import { projectFilesRoots } from "../core/filesRoots.js";
import { projectLayoutCacheId } from "../core/directoryScope.js";
import { projectFilesRpc } from "../core/projectFilesRpc.js";
import { readProjectFilesSupport, PROJECT_FILES_SUPPORT_KIND } from "../core/projectFilesSupport.js";
import { mountProjectRail } from "../core/projectRail.js";
import {
  ALL_WORKSPACES,
  RECLAIMABLE_WORKSPACES,
  projectPageModel,
  workspaceListing,
} from "../core/projectPageModel.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import { subscribeCache } from "../core/localCache.js";
import { askForWorkspaceSizes } from "../core/workspaceSizes.js";
import { readWorkspaceSizeSupport, WORKSPACE_SIZE_SUPPORT_KIND } from "../core/workspaceSizeSupport.js";
import { ICON_PLUS } from "../core/icons.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { mountTasksPane } from "../core/trackerTasksPane.js";
import { hashFromRoute } from "../core/router.js";
import "../styles/tasks.css";
import "../styles/surfaces.css";

/** The project's faces, with Tasks as the default. */
const WORKSPACES_TAB = "workspaces";
const TASKS_TAB = "tasks";
const FILES_TAB = "files";
/// Tasks is the default, so a route that names no tab is on it (#46).
const tabOf = (route) => ([FILES_TAB, WORKSPACES_TAB].includes(route.tab) ? route.tab : TASKS_TAB);

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

/** What the workspace weighs, or the quiet placeholder for a size its machine
 *  is still to send (#273). */
const sizeHtml = (row) =>
  row.sizePending
    ? `<span class="project-size project-size-pending" title="Size not measured yet" aria-label="Size not measured yet">${esc(row.sizeText)}</span>`
    : `<span class="project-size">${esc(row.sizeText)}</span>`;

/** One workspace, in the rail's own row shape: a project page and the rail are
 *  two views of the same list, so they read as the same list. */
const rowHtml = (row, ui) => `<div class="srow inbox-entry project-row${row.muted ? " inbox-muted" : ""}" data-workspace="${esc(row.workspaceKey)}">
    <span class="sdot sdot-${esc(row.state)}" title="${esc(row.state)}"></span>
    <div class="inbox-body">
      <div class="inbox-line inbox-name"><span class="stitle">${esc(row.name)}</span>${unreadHtml(row)}</div>
      <div class="inbox-facts">${esc(factsLine(row))}</div>
      ${lifecycleHtml(row, ui)}
    </div>
    ${sizeHtml(row)}
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
  if (state.tab === FILES_TAB) {
    paintFiles(state, pane);
    return;
  }
  const shown = JSON.stringify([state.page.rows, state.filter, [...state.reclaiming], [...state.reclaimErrors]]);
  if (pane.dataset.rows !== shown) {
    pane.dataset.rows = shown;
    paintWorkspaces(pane, state);
  }
}

/**
 * Open one project face.
 *
 * Each owns the body outright — the workspaces list paints into it and the
 * Tasks pane mounts into it — so the one leaving is torn down before the one
 * arriving is built. The URL is rewritten rather than navigated: the page is
 * the same page, and a navigation would remount the rail beside it.
 */
async function openTab(state, tab) {
  if (state.tab === tab || state.switching) return;
  state.switching = true;
  try {
    if (state.files && !await state.files.canLeave()) return;
    if (state.disposed) return;
    switchTab(state, tab);
  } finally {
    state.switching = false;
  }
}

function switchTab(state, tab) {
  disposeFiles(state);
  state.tasks?.dispose();
  state.tasks = null;
  state.tab = tab;
  askForSizesWhileOpen(state);
  state.rail.paint(tab);
  const pane = $("#project-pane");
  pane.innerHTML = "";
  pane.className = tab === FILES_TAB ? "project-files-page" : "project-page";
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
  void rememberProjectRailRoute(route);
  history.replaceState(null, "", hashFromRoute(route));
}

/** The cached project's folders, always narrowed to the route's machine. */
function projectRecord(state) {
  return state.feed?.projects?.find((project) =>
    project.deviceId === state.context.deviceId && (project.project_id || project.id) === state.route.projectId) || null;
}

function disposeFiles(state) {
  if (App.routeLeaveGuard === state.files?.canLeave) App.routeLeaveGuard = null;
  state.files?.dispose();
  state.files = null;
  state.filesSignature = null;
}

function markFile(state, path, sourceId) {
  const same = state.route.file === path && (!state.route.sourceId || state.route.sourceId === sourceId);
  state.route = { ...state.route, tab: FILES_TAB, file: path, sourceId,
    line: same ? state.route.line : undefined };
  markRoute(state.route);
  void rememberProjectRailRoute(state.route);
}

function paintFilesSupport(state, pane, roots, moved) {
  const note = pane.querySelector("[data-project-files-support]");
  if (!note) return;
  note.textContent = moved ? "A project folder moved. Copy or discard your edits before reopening Files." : "Update the bridge to browse additional project folders.";
  note.hidden = !moved && (state.projectSources || roots.length < 2);
}

/** A feed repaint keeps the explorer, including its editor and keyboard. A
 * changed source roster is picked up once it holds no unsaved file draft. */
function paintFiles(state, pane) {
  const project = projectRecord(state);
  const roots = projectFilesRoots(project, state.route.projectId);
  const signature = JSON.stringify(roots);
  if (signature !== state.filesSignature && !state.files?.hasUnsavedChanges()) {
    disposeFiles(state);
    pane.className = "project-files-page";
    pane.innerHTML = '<p class="dim project-files-support" data-project-files-support>Update the bridge to browse additional project folders.</p><div class="project-files-body"></div>';
    state.filesSignature = signature;
    state.files = renderFilesTab(pane.querySelector(".project-files-body"), {
      roots,
      layoutEntityId: projectLayoutCacheId(state.route.projectId),
      callRpc: projectFilesRpc(state.context, roots[0]?.id, { project, currentProject: () => projectRecord(state) }),
      cacheScope: state.context.cacheScope,
      openAt: state.route.file ? { rootId: state.route.sourceId, path: state.route.file, line: state.route.line || null } : null,
      onFileOpen: (path, sourceId) => markFile(state, path, sourceId),
      viewingContext: App.viewingContext,
    });
    App.routeLeaveGuard = state.files.canLeave;
  }
  paintFilesSupport(state, pane, roots, signature !== state.filesSignature);
}

function watchProjectFilesSupport(state) {
  const read = async () => {
    const support = await readProjectFilesSupport(state.context.deviceId);
    if (state.disposed || support === state.projectSources) return;
    state.projectSources = support;
    paint(state);
  };
  void read();
  return subscribeCache({ deviceId: state.context.deviceId }, (address) => {
    if (address?.kind === PROJECT_FILES_SUPPORT_KIND) void read();
  });
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

/** The Workspaces tab asks its machine for every workspace's size once each
 *  time it opens (#273); the sizes arrive through the feed. Leaving the tab
 *  ends an ask still waiting for the machine to greet. */
function askForSizesWhileOpen(state) {
  state.sizeAsk?.stop();
  state.sizeAsk = state.tab === WORKSPACES_TAB
    ? askForWorkspaceSizes(state.context.deviceId, state.context.rpc, state.route.projectId)
    : null;
}

/** The page as the feed and the cached size support have it. */
function rebuildPage(state) {
  state.page = projectPageModel(state.feed, state.route, { measuresSizes: state.measuresSizes });
}

/** Whether a row with no size yet shows the placeholder is a fact a greeting
 *  writes to the cache; the rows repaint when it lands. */
function watchSizeSupport(state) {
  const read = async () => {
    const measuresSizes = await readWorkspaceSizeSupport(state.context.deviceId);
    if (state.disposed || measuresSizes === state.measuresSizes) return;
    state.measuresSizes = measuresSizes;
    rebuildPage(state);
    paint(state);
  };
  void read();
  return subscribeCache({ deviceId: state.context.deviceId }, (address) => {
    if (address?.kind === WORKSPACE_SIZE_SUPPORT_KIND) void read();
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
  void rememberProjectRailRoute(route);
  // The surface paints what the records hold of this machine whether or not it
  // can answer. Only a machine nothing here has ever held has nothing to paint:
  // the notice names it, waits for it, and hands the link back when it lands.
  if (!context) {
    root.className = "surface";
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const projectSources = await readProjectFilesSupport(context.deviceId);
  if (App.route !== route || !context.active()) return;
  root.className = "surface";
  const state = {
    route, context, disposed: false, selection: shellSelection(),
    page: projectPageModel(null, route), verb: null,
    tab: tabOf(route), view: route.view || "dashboard", feed: null, tasks: null, rail: null,
    reclaiming: new Set(), reclaimErrors: new Map(), filter: ALL_WORKSPACES,
    measuresSizes: false, sizeAsk: null, files: null, filesSignature: null, switching: false, projectSources,
  };
  state.verb = (host) => paintProjectVerbs(host, state);
  root.innerHTML = `<div id="tabbody" class="flush"><div id="project-pane" class="project-page"></div></div>`;
  // The three tabs are the rail's faces. A press switches in place rather than
  // navigating, because a navigation would remount the agent rail beside the
  // page; the shell's route rule never sees it (#62), so where the chat lies
  // over the page the press puts it away itself.
  state.rail = mountProjectRail($("#dir-rail"), {
    route,
    context,
    navigate: go,
    onSelect: (tab) => {
      collapseChatOverPage();
      void openTab(state, tab);
    },
  });
  state.rail.paint(state.tab);
  $("#project-pane").onclick = (event) => pressList(state, event);
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => !state.page.empty });
  const unsubscribe = subscribeFeed((feed) => {
    if (state.disposed) return;
    state.feed = feed;
    rebuildPage(state);
    paint(state);
  });
  const unwatchSizeSupport = watchSizeSupport(state);
  const unwatchFilesSupport = watchProjectFilesSupport(state);
  askForSizesWhileOpen(state);
  if (state.tab === TASKS_TAB) mountTasks(state, $("#project-pane"));
  paint(state);
  App.viewDispose = () => {
    state.disposed = true;
    unsubscribe();
    unwatchSizeSupport();
    unwatchFilesSupport();
    disposeFiles(state);
    state.sizeAsk?.stop();
    deviceStrip();
    state.tasks?.dispose();
    clearToolbarVerb(state.verb);
    state.rail.dispose();
  };
}
