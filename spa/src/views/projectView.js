// The project's own surface: what the project holds, and who you talk to about
// it.
//
// A project is a template — its checkout is the base every workspace is cut
// FROM, and nothing opens it — so this page is about the project itself. The
// main pane is the project's workspaces, each row opening its own surface; the
// rail is the project's agent, whose conversation the bridge keeps in a scratch
// directory of its own (planning/v2/workspaces.md).
//
// The page's two verbs sit in the toolbar's verb slot beside the project's
// name: the + that makes the first workspace, and the cog that settles the
// project. Both say which project they are about, because the bar names one.
//
// Two tabs, because a project holds two kinds of thing: the workspaces the work
// happens in, and the issues that say what the work IS. The Issues tab is its
// own surface (core/trackerIssuesPane.js) mounted into this page's body, and it
// keeps its own URL — `#/project/<p>/issues` — so a link to a board opens one.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { esc } from "../core/text.js";
import { shellSelection } from "../core/shell.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { clearProjectTabHandler, clearToolbarVerb, setProjectTabHandler, setToolbarVerb } from "../core/toolbar.js";
import { openCreateWork } from "../core/createWork.js";
import { openProjectSettings } from "../sheets/projectSettings.js";
import { projectPageModel } from "../core/projectPageModel.js";
import { refreshFeed, subscribeFeed } from "../core/taskFeed.js";
import { ICON_PLUS, ICON_SETTINGS } from "../core/icons.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { mountIssuesPane } from "../core/trackerIssuesPane.js";
import { hashFromRoute } from "../core/router.js";
import "../styles/issues.css";
import "../styles/surfaces.css";

/** The two tabs, and which one a route stands on. Workspaces is the page
 *  itself, so a route that names no tab names that one. */
const WORKSPACES_TAB = "workspaces";
const ISSUES_TAB = "issues";
/// Issues is the default, so a route that names no tab is on it (#46).
const tabOf = (route) => (route.tab === WORKSPACES_TAB ? WORKSPACES_TAB : ISSUES_TAB);

/** Line two of a workspace row: what it is standing on, what its checkout is
 *  doing when that is not simply "ready", and what the work weighs. The same
 *  three facts the rail's rows carry, with the room this page has for them. */
const factsLine = (row) =>
  [row.branch, row.status === "ready" ? "" : row.statusText, row.facts].filter(Boolean).join(" · ");

const unreadHtml = (row) => (row.unreadCount > 0 ? `<span class="badge inbox-unread">${row.unreadCount}</span>` : "");

/** One workspace, in the rail's own row shape: a project page and the rail are
 *  two views of the same list, so they read as the same list. */
const rowHtml = (row) => `<div class="srow inbox-entry project-row${row.muted ? " inbox-muted" : ""}" data-workspace="${esc(row.workspaceKey)}">
    <span class="sdot sdot-${esc(row.state)}" title="${esc(row.state)}"></span>
    <div class="inbox-body">
      <div class="inbox-line inbox-name"><span class="stitle">${esc(row.name)}</span>${unreadHtml(row)}</div>
      <div class="inbox-facts">${esc(factsLine(row))}</div>
    </div>
  </div>`;

/** A project nobody has cut a workspace in yet. That is the first state of
 *  every project, not a broken one, so the page says what to do about it and
 *  points at the control that does it. */
const emptyHtml = () => `<div class="empty project-empty">
    <h2>No workspaces yet</h2>
    <p>A workspace is where the work happens in this project. The + above makes the first one.</p>
  </div>`;

const pageHtml = (page) =>
  page.empty ? emptyHtml() : `<div class="project-rows">${page.rows.map(rowHtml).join("")}</div>`;

/** The two verbs this page owns, in the toolbar's slot. Called on every toolbar
 *  repaint, so it rebuilds only when the project it names has changed. */
function paintProjectVerbs(host, state) {
  const name = state.page.name;
  if (host.dataset.project === name && host.querySelector("[data-project-create]")) return;
  host.dataset.project = name;
  host.innerHTML = `<button class="iconbtn" type="button" data-project-settings aria-label="Settings for ${esc(name)}" title="Settings for ${esc(name)}">${ICON_SETTINGS}</button>
    <button class="iconbtn" type="button" data-project-create aria-label="New workspace in ${esc(name)}" title="New workspace in ${esc(name)}">${ICON_PLUS}</button>`;
  host.querySelector("[data-project-settings]").onclick = () => settleProject(state);
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

/** The cog. A deleted project has no page left to stand on, so the reader is
 *  put back on the inbox and the feed is told to catch up. */
function settleProject(state) {
  openProjectSettings(state.route.projectId, {
    callRpc: state.context.rpc,
    deviceId: state.context.deviceId,
    onDeleted: async () => {
      go({ name: "inbox" });
      await refreshFeed(state.context.deviceId);
    },
  });
}

function paint(state) {
  setToolbarVerb(state.verb);
  const pane = $("#project-pane");
  if (!pane) return;
  if (state.tab === ISSUES_TAB) {
    state.issues?.feedMoved();
    return;
  }
  const shown = JSON.stringify(state.page.rows);
  if (pane.dataset.rows !== shown) {
    pane.dataset.rows = shown;
    pane.innerHTML = pageHtml(state.page);
  }
}

/**
 * Open one of the two tabs.
 *
 * Each owns the body outright — the workspaces list paints into it and the
 * Issues pane mounts into it — so the one leaving is torn down before the one
 * arriving is built. The URL is rewritten rather than navigated: the page is
 * the same page, and a navigation would remount the rail beside it.
 */
function openTab(state, tab) {
  if (state.tab === tab) return;
  state.issues?.dispose();
  state.issues = null;
  state.tab = tab;
  const pane = $("#project-pane");
  pane.innerHTML = "";
  delete pane.dataset.rows;
  if (tab === ISSUES_TAB) mountIssues(state, pane);
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

function mountIssues(state, pane) {
  state.issues = mountIssuesPane(pane, {
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

/** A row opens its workspace. The row carries the route the rail would have
 *  opened it with, so the same workspace opens the same way from either list. */
function openWorkspace(state, workspaceKey) {
  const row = state.page.rows.find((candidate) => candidate.workspaceKey === workspaceKey);
  if (row?.route) go(row.route);
}

export async function renderProject() {
  const root = $("#root");
  const route = App.route;
  const context = routeContext(route);
  root.className = "surface";
  // A machine that cannot answer — never opened here, or gone since — has
  // nothing under this link to read or write, so the surface names it rather
  // than standing a frame up over calls that can only be refused.
  if (!canAnswer(context)) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const state = {
    route, context, disposed: false, selection: shellSelection(),
    page: projectPageModel(null, route), verb: null,
    tab: tabOf(route), view: route.view || "dashboard", feed: null, issues: null, openTab: null,
  };
  state.verb = (host) => paintProjectVerbs(host, state);
  root.innerHTML = `<div id="tabbody" class="flush"><div id="project-pane" class="project-page"></div></div>`;
  // The two tabs are the toolbar's (core/toolbar.js), so they stay reachable
  // with the chat open over the page; a press is handed here to switch in
  // place, because a navigation would remount the rail beside the page.
  state.openTab = (tab) => openTab(state, tab === WORKSPACES_TAB ? WORKSPACES_TAB : ISSUES_TAB);
  setProjectTabHandler(state.openTab);
  $("#project-pane").onclick = (event) => {
    const row = event.target.closest("[data-workspace]");
    if (row) openWorkspace(state, row.dataset.workspace);
  };
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => !state.page.empty });
  const unsubscribe = subscribeFeed((feed) => {
    if (state.disposed) return;
    state.feed = feed;
    state.page = projectPageModel(feed, state.route);
    paint(state);
  });
  if (state.tab === ISSUES_TAB) mountIssues(state, $("#project-pane"));
  paint(state);
  App.viewDispose = () => {
    state.disposed = true;
    unsubscribe();
    deviceStrip();
    state.issues?.dispose();
    clearToolbarVerb(state.verb);
    clearProjectTabHandler(state.openTab);
  };
}
