// A project's navigation, on the shell's rail (#dir-rail): Tasks and
// Workspaces, with the project's Settings at the foot (#274).
//
// "The project view should use a left rail the way a workspace does, with three
// entries only: Workspaces, Tasks and the project's Settings." They used to be
// two tabs after the project's name in the toolbar and a cog in its verb slot.
//
// core/directoryRail.js draws the cells; this is what a project hangs on them —
// the Tasks count, the unread of every watched task in the project (#104), kept
// on the cell across repaints. It is read off the cached task list
// (core/projectTasksUnread.js), so the rail paints from the cache and never asks
// whether the machine is answering.
//
// The project page stands it up and so does a task's page, which is a page OF
// the Tasks face: the rail is how the reader gets back to the list.

import { PROJECT_TABS, paintDirectoryRail } from "./directoryRail.js";
import { followProjectTasksUnread } from "./projectTasksUnread.js";
import { projectSettingsLabel } from "./text.js";
import { refreshFeed } from "./taskFeed.js";
import { openProjectSettings } from "../sheets/projectSettings.js";

/** The cog's sheet, on the project the route names and the machine it is on.
 *  A deleted project has no page left to stand on, so the reader is put back
 *  on the inbox and the feed is told to catch up. */
export function openRouteProjectSettings(route, context, { navigate }) {
  openProjectSettings(route.projectId, {
    callRpc: context.rpc,
    deviceId: context.deviceId,
    onDeleted: async () => {
      navigate({ name: "inbox" });
      await refreshFeed(context.deviceId);
    },
  });
}

/**
 * mountProjectRail(host, { route, context, onSelect, navigate }) — the rail for
 * one project route, `context` the route's machine (core/surfaceContext.js).
 * Returns { paint(active), dispose() }: a paint draws the faces with `active`
 * marked and says the Tasks count on the cell it drew.
 */
export function mountProjectRail(host, { route, context, onSelect, navigate }) {
  const settings = { onOpen: () => openRouteProjectSettings(route, context, { navigate }), label: projectSettingsLabel };
  const tasks = followProjectTasksUnread(() => sayCount());

  function sayCount() {
    const badge = host.querySelector("[data-tab=tasks] .dirtab-count");
    const count = tasks.count();
    if (badge) badge.textContent = count ? String(count) : "";
  }

  tasks.follow(route.deviceId, route.projectId);

  return {
    paint(active) {
      paintDirectoryRail(host, { tabs: PROJECT_TABS, active, onSelect, settings, sidebar: false });
      sayCount();
    },
    // The rail is the shell's column, lent to whichever surface is standing on
    // it: leaving hands it back empty.
    dispose() {
      tasks.dispose();
      host.innerHTML = "";
    },
  };
}
