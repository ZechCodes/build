// Badges describe the current cached roster, never the model at message-write.
import { modelFamilyName, shortModelLabel } from "./agentChoice.js";
import { esc } from "./text.js";
import { freshestRosters } from "./inboxRoster.js";
import { entityIdOf } from "./entityId.js";

const modelId = (agent) => agent?.active_model || agent?.model || "";
const effortOf = (agent) => agent?.active_effort || agent?.effort || agent?.reasoningEffort;

export function agentModelBadge(agent, catalog = null) {
  const id = modelId(agent);
  const title = shortModelLabel(catalog, agent?.provider, id);
  const family = modelFamilyName(title);
  if (!family) return null;
  const effort = effortOf(agent);
  return { family, title: effort ? `${title} · ${effort}` : title };
}

export function agentModelBadgeHtml(agent, { catalog = null, className = "" } = {}) {
  const badge = agentModelBadge(agent, catalog);
  return badge ? `<span class="agent-model-badge${className ? ` ${esc(className)}` : ""}" title="${esc(badge.title)}">${esc(badge.family)}</span>` : "";
}

export const agentFromGroups = (groups, id) =>
  (groups || []).flatMap((group) => group.agents || []).find((agent) => agent.id === id);

/** Both project and workspace agents, as the already cached feed knows them. */
export function agentModelsFromFeed(feed, projectKey) {
  const rows = [...(feed?.projects || []), ...(feed?.runs || []), ...(feed?.items || [])]
    .filter((row) => row.projectKey === projectKey);
  const rosterOf = freshestRosters(rows);
  const newest = rows.map((row) => entityIdOf(row) ? rosterOf(projectKey, entityIdOf(row)) : row);
  const agents = newest.flatMap((row) => row?.agents || []);
  const projectAgents = (feed?.projects || []).filter((project) => project.projectKey === projectKey)
    .flatMap((project) => project.projectRoster?.agents || []);
  return Object.fromEntries([...agents, ...projectAgents].map((agent) => [agent.id, agent]));
}
