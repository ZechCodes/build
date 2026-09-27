// A workspace's navigation, on the shell's rail (#dir-rail): Changes, Files and
// the Tasks its agents hold, with the workspace's Settings at the foot (#174).
//
// "Tasks moved to the rail as an icon along with the settings. Making the left
// rail the workspace navigation." The rail is workspace scoped: it stands on the
// route alone, before the workspace's record has landed, and which directory
// Changes or Files is looking at is said inside the pane, below it.
//
// core/directoryRail.js draws the cells; this is what a workspace hangs on them
// — the Tasks count (core/trackerWorkspaceTasksView.js), kept on the cell
// across repaints, and the settings sheet the toolbar's cog used to open.

import { WORKSPACE_TABS, paintDirectoryRail } from "./directoryRail.js";
import { mountWorkspaceTasks } from "./trackerWorkspaceTasksView.js";
import { workspaceAgents } from "./trackerAssignee.js";
import { routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { canAnswer, contextFor } from "./deviceContexts.js";
import { deviceCatalog } from "./inboxDevices.js";
import { notifyError } from "./notify.js";
import { refreshFeed } from "./taskFeed.js";
import { workspaceDisplayName } from "./workspaceModel.js";
import { openWorkspaceSettings } from "../sheets/workspaceSettings.js";

/** The agents standing in the workspace, in the order its row lists them — the
 *  same order the agent rail's bubbles read across. Read at every count rather
 *  than captured: a workspace gains and loses agents while the rail stands. */
const agentsOf = (feed, route) => {
  const group = workspaceAgents(feed(), routeProjectKey(route)).find((candidate) => candidate.workspaceId === route.workspaceId);
  return (group?.agents || []).map((agent, index) => ({ id: agent.id, ordinal: index + 1 }));
};

/**
 * The cog's sheet, on the workspace the route is standing in and the machine
 * that workspace is on. The record carries the name; one that has not landed
 * yet still has the id the URL carries, so the sheet opens either way.
 */
export function openRouteWorkspaceSettings(route, workspace, { navigate }) {
  const { deviceId, workspaceId } = route;
  const context = contextFor(deviceId);
  if (!canAnswer(context)) {
    notifyError("That machine is not reachable.", "Workspace settings are read and written on the device the workspace is on.");
    return;
  }
  openWorkspaceSettings(
    { id: workspaceId, name: workspaceDisplayName(workspace, workspaceId), workspaceKey: routeWorkspaceKey(route) },
    {
      callRpc: context.rpc,
      deviceId,
      catalog: deviceCatalog(deviceId),
      // The name is printed by the toolbar's picker and by every inbox row,
      // both of which read the feed this refresh rewrites.
      onRenamed: () => refreshFeed(deviceId),
      // Standing in a workspace that no longer exists is standing nowhere.
      onDeleted: async () => {
        navigate({ name: "inbox" });
        await refreshFeed(deviceId);
      },
    },
  );
}

/**
 * mountWorkspaceRail(host, { route, feed, workspace, onSelect, navigate }) —
 * the rail for one workspace route. `feed()` is the route's machine's feed and
 * `workspace()` its record (null until it lands), both read at use. Returns
 * { paint(active), feedMoved(), dispose() }: a paint draws the faces with
 * `active` marked, and hands the Tasks count the cell it drew.
 */
export function mountWorkspaceRail(host, { route, feed, workspace, onSelect, navigate }) {
  let tasks = null;
  const settings = { onOpen: () => openRouteWorkspaceSettings(route, workspace(), { navigate }) };

  const keepTasksCount = () => {
    const cell = host.querySelector("[data-tab=tasks]");
    if (tasks) tasks.retarget(cell);
    else tasks = mountWorkspaceTasks(cell, { deviceId: route.deviceId, projectId: route.projectId, agents: () => agentsOf(feed, route) });
  };

  return {
    paint(active) {
      paintDirectoryRail(host, { tabs: WORKSPACE_TABS, active, onSelect, settings });
      keepTasksCount();
    },
    feedMoved: () => tasks?.refresh(),
    // The rail is the shell's column, lent to whichever surface is standing on
    // it: leaving hands it back empty.
    dispose() {
      tasks?.dispose();
      tasks = null;
      host.innerHTML = "";
    },
  };
}
