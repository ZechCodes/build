import { deviceKey } from "./deviceKey.js";

export const projectIsArchived = project => project?.archived_at != null;
const projectKeyOf = project => project.projectKey || deviceKey(project.deviceId, project.project_id || project.id);
export function visibleProjectRows(rows, projects) {
  const archived = new Set(projects.filter(projectIsArchived).map(projectKeyOf));
  return rows.filter(row => !archived.has(row.projectKey || deviceKey(row.deviceId, row.project_id)));
}
