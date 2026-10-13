// Complete cached project rosters replace the legacy pinned-only owner rows.
// Workspace rows retain their own agents and residence.
import { entityIdOf } from "./entityId.js";

const matchesOwner = (row, project) => entityIdOf(row) === project.projectRoster.entity_id
  && (row.projectKey && project.projectKey
    ? row.projectKey === project.projectKey
    : row.project_id === (project.id || project.project_id));

export function projectRosterRows(feed) {
  const rows = feed?.items || [];
  const projects = (feed?.projects || []).filter((project) => project.projectRoster);
  const otherRows = rows.filter((row) => !projects.some((project) => matchesOwner(row, project)));
  const ownerRows = projects.map((project) => ({
    ...rows.find((row) => matchesOwner(row, project)),
    project_id: project.id || project.project_id,
    projectKey: project.projectKey,
    ...project.projectRoster,
  }));
  return [...otherRows, ...ownerRows];
}


/** Worker-capable rosters name their coordinator by persisted identity.
 *  Earlier snapshots retain their historical pinned-only holder rule. */
export function projectCoordinatorAgent(project, agents = []) {
  if (project.projectRoster) return agents.find((agent) => agent.id === project.projectRoster.pinned_agent_id) || null;
  return agents.find((agent) => agent.working === true) || agents[0] || null;
}
