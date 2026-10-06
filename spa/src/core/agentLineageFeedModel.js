// Descendant activity on one device's feed snapshot, derived from its cached
// rosters. The snapshot stays separate from the replicas it was read from.
import { agentLineage, lineageMembers, withRollup } from "./agentLineageModel.js";
import { entityIdOf } from "./entityId.js";
import { freshestRosters } from "./inboxRoster.js";

/** Use each conversation's newest roster, including runs absent from items. */
function newestRosters(rows) {
  const rosterOf = freshestRosters(rows);
  const named = new Map(rows.filter((row) => entityIdOf(row)).map((row) => [entityIdOf(row), row]));
  return [...named.values()].map((row) => rosterOf(row.projectKey, entityIdOf(row))).filter(Boolean);
}

/** Each project gets its own lineage: an id or creator in another project
 * cannot make this project's parent run. The caller already scopes devices. */
function projectLineages(snapshot, namesMakers) {
  const rows = [...(snapshot.runs || []), ...snapshot.items];
  const projectIds = new Set(rows.map((row) => row.project_id));
  return new Map([...projectIds].map((projectId) => {
    const members = lineageMembers(newestRosters(rows.filter((row) => row.project_id === projectId)), {
      workspaces: snapshot.workspaces, projects: snapshot.projects,
    });
    return [projectId, agentLineage(members, { namesMakers })];
  }));
}

/** Add running rollups without changing each agent's own working flag or
 * writing the derived digests back to disk. Spreads carry cache freshness. */
export function withFeedAgentRollups(snapshot, { namesMakers = false } = {}) {
  if (!snapshot) return snapshot;
  const lineages = projectLineages(snapshot, namesMakers);
  const decorate = (row) => Array.isArray(row.agents)
    ? { ...row, agents: row.agents.map((agent) => withRollup(agent, lineages.get(row.project_id))) } : row;
  return { ...snapshot, items: snapshot.items.map(decorate), runs: (snapshot.runs || []).map(decorate) };
}
