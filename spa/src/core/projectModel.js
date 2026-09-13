/** The primary checkout route for a project returned by project.add/list.
 * Plain folders start in Files; repositories retain the Changes landing. */
export function projectRoute(project) {
  const projectId = project && (project.project_id || project.id);
  if (!projectId) return null;
  return {
    name: "branch",
    projectId,
    branch: project.base_branch || "main",
    tab: project.is_git === false ? "files" : "changes",
  };
}

/** The first useful directory in a workspace, preferring a repository because
 * Changes is the most informative landing surface when one is available. */
export function workspaceRoute(workspace) {
  if (!workspace?.id || !workspace.project_id) return null;
  const directories = workspace.directories || [];
  const directory = directories.find((entry) => entry.is_git) || directories[0];
  return {
    name: "workspace",
    projectId: workspace.project_id,
    workspaceId: workspace.id,
    ...(directory ? { sourceId: directory.source_id || directory.id } : null),
    tab: directory?.is_git === false ? "files" : "changes",
  };
}
