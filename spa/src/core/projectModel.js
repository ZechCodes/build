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
