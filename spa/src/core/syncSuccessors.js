import { entityIdOf } from "./entityId.js";
// Record families match subscription coverage. Only reads dispatched before a
// subscription was held can leave a gap; families still ahead need no repeat.
const INBOX_GROUPS = ["lists", "rosters", "threads", "tasks"];
export const readsSyncGroup = (turn, group) => !turn.groups || turn.groups.has(group);

export function syncReadGroup(method) {
  if (["board.list", "project.list", "workspace.list"].includes(method)) return "lists";
  if (method === "agent.list") return "rosters";
  if (method.startsWith("thread.")) return "threads";
  if (method.startsWith("tasks.")) return "tasks";
  return "checkout";
}

export function requiredSuccessorGroups(observed, reason, scoped = false) {
  if (reason === "connection") return new Set();
  if (reason === "read-state") return new Set(observed.has("lists") ? ["lists"] : []);
  if (reason === "subscription-inbox") return new Set(INBOX_GROUPS.filter((group) => observed.has(group)));
  if (reason === "subscription-background") return new Set(observed.has("checkout") ? ["checkout"] : []);
  return observed.size || scoped ? null : new Set();
}

export function queueSuccessor(turn, groups) {
  if (groups?.size === 0) return false;
  turn.again = true;
  if (groups === null || turn.successorGroups === null) turn.successorGroups = null;
  else turn.successorGroups = new Set([...(turn.successorGroups || []), ...groups]);
  return true;
}

export const discoveredThreadKey = (entityId, agent) => JSON.stringify([entityId, agent.conversation_id || agent.id]);
export const emptySyncDiscovery = () => ({ checkout: new Set(), threads: new Set(), tasks: new Set(), rosters: new Set() });

export function discoverRosterThreads(discovery, entityId, before, agents) {
  const known = new Map((before || []).map((agent) => [discoveredThreadKey(entityId, agent), agent]));
  for (const agent of agents || []) {
    const key = discoveredThreadKey(entityId, agent);
    const held = known.get(key);
    if (!held || held.thread_id !== agent.thread_id || held.thread_generation_revision !== agent.thread_generation_revision) discovery.threads.add(key);
  }
}

const projectId = (project) => project.project_id || project.id;
const ownerId = (project) => project.entity_id || project.run_id;

export function discoverSyncOwnership(discovery, before, after, beforeRows, afterRows) {
  const knownProjects = new Set((before.projects || []).map(projectId));
  const knownOwners = new Set((before.projects || []).map(ownerId));
  for (const project of after.projects || []) {
    if (!knownProjects.has(projectId(project))) discovery.tasks.add(projectId(project));
    if (ownerId(project) && !knownOwners.has(ownerId(project))) discovery.rosters.add(ownerId(project));
  }
  const knownRows = new Map(beforeRows.map((row) => [entityIdOf(row), row]));
  for (const row of afterRows) discoverRow(discovery, knownRows, row);
}

function discoverRow(discovery, knownRows, row) {
  const entityId = entityIdOf(row);
  if (!entityId) return;
  const before = knownRows.get(entityId);
  if (!before || checkoutIdentity(before) !== checkoutIdentity(row)) discovery.checkout.add(entityId);
  discoverRosterThreads(discovery, entityId, before?.agents, row.agents);
}

const checkoutIdentity = (row) => JSON.stringify([row.run_id, row.project_id, row.worktree_id, row.workspace_id]);
