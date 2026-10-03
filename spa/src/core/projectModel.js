import { directoryTab } from "./workspaceModel.js";

/** Where an ordinary project link opens: its Tasks face and the agent you
 * talk to about it. Explicit toolbar returns use projectRailState instead.
 *
 * The project page also offers its original sources in Files and the
 * workspaces cut from them (core/router.js writes it as `#/project/<id>`).
 *
 * The route names the machine the project is on, since every machine mints a
 * `proj-1` of its own. A project the feed has not stamped — the answer to a
 * fresh project.create, which came off one bridge and knows it — names none, and
 * its caller stamps the device it asked. */
export function projectRoute(project) {
  const projectId = project && (project.project_id || project.id);
  if (!projectId) return { name: "inbox" };
  return { name: "project", ...(project.deviceId ? { deviceId: project.deviceId } : null), projectId };
}

/** The first useful directory in a workspace, preferring a repository because
 * Changes is the most informative landing surface when one is available.
 *
 * The route names the machine the workspace is checked out on, for the same
 * reason a project route does. A workspace the feed has not stamped — the answer
 * to a fresh workspace.create — names none, and its caller stamps the device it
 * asked. */
export function workspaceRoute(workspace) {
  if (!workspace?.id || !workspace.project_id) return null;
  const directories = workspace.directories || [];
  const directory = directories.find((entry) => entry.is_git) || directories[0];
  return {
    name: "workspace",
    deviceId: workspace.deviceId,
    projectId: workspace.project_id,
    workspaceId: workspace.id,
    ...(directory ? { sourceId: directory.source_id || directory.id } : null),
    tab: directoryTab(directory),
  };
}
