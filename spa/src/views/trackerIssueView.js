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
// The project's two tabs, with Issues open, are the toolbar's (core/toolbar.js
// draws them for this route): an issue is a page OF the issues tab, and the
// bar is where they stay reachable with the chat open over the page.
//
// The project's agent stays beside an issue of the project, as it is beside the
// project page — and it is the SHELL that keeps it there (core/shell.js), which
// is why this file no longer has to remember one. It used to forget: this page
// mounted no rail at all, so opening an issue on a phone lost the bubble strip
// and the bar with it. Standing on the project rather than on the issue is also
// what makes opening an issue from the Issues tab leave the strip alone.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { mountIssuePage } from "../core/trackerIssuePage.js";
import { issueContextItem } from "../core/trackerViewingContext.js";
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
  root.innerHTML = `<div id="tabbody" class="flush"><div id="issue-pane" class="issue-surface"></div></div>`;
  let feed = null;
  /**
   * Say which issue is on screen, so the project agent's rail beside it knows
   * what the reader is looking at.
   *
   * Set from the READ rather than from the route: the route names an id, and
   * an agent told an id and nothing else is no better off. Gated on the
   * bridge — a kind an older one does not know is refused, and the refusal
   * takes the reader's message with it (core/trackerViewingContext.js).
   */
  const sayWhichIssue = (issue) => {
    const item = issueContextItem(issue, context.deviceId);
    if (item) App.viewingContext?.set?.({ version: 1, items: [item] });
  };
  const page = mountIssuePage($("#issue-pane"), {
    onIssueRead: sayWhichIssue,
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
  };
}

