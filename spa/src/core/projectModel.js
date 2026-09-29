import { directoryTab } from "./workspaceModel.js";

/** Where a project itself opens: its own page — the workspaces it holds, and
 * the agent you talk to about it.
 *
 * A project is a template, never a place to work: its own checkout is the base
 * the workspaces are cut from, and no surface opens it. The page is about the
 * project, and the work is in the workspaces on it (core/router.js writes it as
 * `#/project/<id>`).
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
