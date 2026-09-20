// The issue page's route host: `#/project/<p>/issues/<issueId>`.
//
// Thin by design — the surface itself is core/trackerIssuePage.js. This file is
// what the route needs: the machine the project is on, its caller and its
// catalog, the feed the links and the assignee names are read off, and the
// teardown.
//
// An issue belongs to exactly one project and never moves between projects, so
// the project in the URL is the project — there is no lookup to do here beyond
// the device one the router already parked on.
//
// The project's two tabs stand over the page exactly as they stand over the
// project page, with Issues open: an issue is a page OF the issues tab, and
// the tabs are how you get back to the list, or across to the workspaces,
// from a phone that has no other way back.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { mountIssuePage } from "../core/trackerIssuePage.js";
import { mountTabShell } from "../core/tabshell.js";
import "../styles/issues.css";
import "../styles/surfaces.css";

export async function renderTrackerIssue() {
  const root = $("#root");
  const route = App.route;
  const context = routeContext(route);
  root.className = "surface";
  // A machine that cannot answer — never opened here, or gone since — has
  // nothing under this link to read or write, so the surface names it rather
  // than standing a page up over calls that can only be refused.
  if (!canAnswer(context)) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  root.innerHTML = `<div id="project-tabs"></div><div id="tabbody" class="flush"><div id="issue-pane" class="issue-surface"></div></div>`;
  const tabs = mountTabShell($("#project-tabs"), {
    tabs: [{ id: WORKSPACES_TAB, label: "Workspaces" }, { id: ISSUES_TAB, label: "Issues" }],
    active: ISSUES_TAB,
    onSelect: (tab) => void go(projectTabRoute(route, tab)),
  });
  let feed = null;
  const page = mountIssuePage($("#issue-pane"), {
    projectId: route.projectId,
    deviceId: context.deviceId,
    projectKey: routeProjectKey(route),
    issueId: route.issueId,
    callRpc: context.rpc,
    catalog: () => context.modelCatalog(),
    refreshCatalog: () => context.refreshModelCatalog(),
    feed: () => feed,
    navigate: go,
  });
  const deviceStrip = mountDeviceStrip(root, context, { hasContent: () => true });
  const unsubscribe = subscribeFeed((snapshot) => {
    feed = snapshot;
    page.feedMoved();
  });
  App.viewDispose = () => {
    unsubscribe();
    deviceStrip();
    page.dispose();
    tabs.dispose?.();
  };
}

const WORKSPACES_TAB = "workspaces";
const ISSUES_TAB = "issues";

/** The project page this issue is under, on the tab that was pressed: the
 *  same machine and project, the issue dropped, Issues named where that is
 *  the tab (core/router.js reads the tab off the route). */
function projectTabRoute(route, tab) {
  const { issueId: _issueId, ...project } = route;
  return { ...project, name: "project", ...(tab === ISSUES_TAB ? { tab: ISSUES_TAB } : {}) };
}
