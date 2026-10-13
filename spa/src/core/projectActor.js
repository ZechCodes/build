// Purpose comes from exact cached membership or a durable task identity.
// The legacy prefix stands in only when those records cannot name a worker.
const hasWorkerName = (id, context, identity) => Boolean(identity?.name
  || context.identities?.[id]?.name || context.agentLabels?.[id]);

export function isProjectWorker(id, context = {}, identity = null) {
  const groups = context.agentGroups || [];
  if (groups.some((group) => group.pinnedAgentId === id)) return false;
  if (groups.some((group) => group.kind === "project" && group.agents?.some((agent) => agent.id === id))) return true;
  return hasWorkerName(id, context, identity);
}

export const isProjectActor = (actor, context = {}) => actor?.kind === "project_agent"
  || (String(actor?.agent_id || "").startsWith("project-") && !isProjectWorker(actor.agent_id, context, actor.identity));
