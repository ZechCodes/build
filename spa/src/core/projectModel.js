/** The primary checkout route for a project returned by project.add/list.
 * Plain folders start in Files; repositories retain the Changes landing.
 *
 * The route names the machine the project is on, since every machine mints a
 * `proj-1` of its own. A project the feed has not stamped — the answer to a
 * fresh project.add, which came off one bridge and knows it — names none, and
 * the app looks it up (core/routeResolve.js) rather than inventing one. */
export function projectRoute(project) {
  const projectId = project && (project.project_id || project.id);
  if (!projectId) return null;
  return {
    name: "branch",
    deviceId: project.deviceId,
    projectId,
    branch: project.base_branch || "main",
    tab: project.is_git === false ? "files" : "changes",
  };
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
    tab: directory?.is_git === false ? "files" : "changes",
  };
}
