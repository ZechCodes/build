// The task page's route host: `#/project/<p>/tasks/<taskId>`.
//
// Thin by design — the surface itself is core/trackerTaskPage.js. This file is
// what the route needs: the machine the project is on, its caller and its
// catalog, the feed the links and the assignee names are read off, and the
// teardown.
//
// A task belongs to exactly one project and never moves between projects, so
// the project in the URL is the project — there is no lookup to do here beyond
// the device one the router already parked on.
//
// The project's two tabs, with Tasks open, are the toolbar's (core/toolbar.js
// draws them for this route): a task is a page OF the tasks tab, and the
// bar is where they stay reachable with the chat open over the page.
//
// The project's agent stays beside a task of the project, as it is beside the
// project page — and it is the SHELL that keeps it there (core/shell.js), which
// is why this file no longer has to remember one. It used to forget: this page
// mounted no rail at all, so opening a task on a phone lost the bubble strip
// and the bar with it. Standing on the project rather than on the task is also
// what makes opening a task from the Tasks tab leave the strip alone.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { mountDeviceNotice, mountDeviceStrip } from "../core/deviceNotice.js";
import { routeProjectKey } from "../core/deviceKey.js";
import { surfaceContext } from "../core/surfaceContext.js";
import { subscribeFeed } from "../core/taskFeed.js";
import { mountTaskPage } from "../core/trackerTaskPage.js";
import { taskContextItem } from "../core/trackerViewingContext.js";
import "../styles/tasks.css";
import "../styles/surfaces.css";

export async function renderTrackerTask() {
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
  root.innerHTML = `<div id="tabbody" class="flush"><div id="task-pane" class="task-surface"></div></div>`;
  let feed = null;
  /**
   * Say which task is on screen, so the project agent's rail beside it knows
   * what the reader is looking at.
   *
   * Set from the READ rather than from the route: the route names an id, and
   * an agent told an id and nothing else is no better off. Gated on the
   * bridge — a kind an older one does not know is refused, and the refusal
   * takes the reader's message with it (core/trackerViewingContext.js).
   */
  const sayWhichTask = (task) => {
    const item = taskContextItem(task, context.deviceId);
    if (item) App.viewingContext?.set?.({ version: 1, items: [item] });
  };
  const page = mountTaskPage($("#task-pane"), {
    onTaskRead: sayWhichTask,
    projectId: route.projectId,
    deviceId: context.deviceId,
    projectKey: routeProjectKey(route),
    taskId: route.taskId,
    commentId: route.commentId || null,
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
