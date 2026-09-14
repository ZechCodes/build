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
