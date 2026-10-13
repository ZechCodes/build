import { cachedWriteOf, captureCachedRecord, isCachedWrite, mergeCachedAtomically, mergeCachedRecordsTogether, readCached, readCachedMany, recordWriteOf } from "./localCache.js";

export const PROJECT_AGENTS_KIND = "project-agents";
export const PROJECT_AGENTS_SUPPORT_KIND = "project-agents-support";
export const projectAgentsAddress = (deviceId, entityId) => ({ deviceId, entityId, kind: PROJECT_AGENTS_KIND });
export const projectAgentsSupportAddress = (deviceId) => ({ deviceId, entityId: "", kind: PROJECT_AGENTS_SUPPORT_KIND });

export function rememberProjectAgentsSupport(deviceId, capabilities, active = () => true) {
  if (!deviceId) return Promise.resolve(false);
  const support = { projectWorkers: capabilities?.agents?.projectWorkers === true,
    assignProjectAgent: capabilities?.tasks?.assignProjectAgent === true };
  return mergeCachedAtomically(projectAgentsSupportAddress(deviceId), (held) =>
    active() && JSON.stringify(held) !== JSON.stringify(support) ? support : null);
}

export async function readProjectAgentsSupport(deviceId) {
  return (await readCached(projectAgentsSupportAddress(deviceId)))?.value || {};
}

export const validProjectRoster = (roster, entityId) => typeof entityId === "string" && entityId.length > 0 && roster?.entity_id === entityId
  && typeof roster.pinned_agent_id === "string"
  && Array.isArray(roster.agents)
  && roster.agents.some((agent) => agent.id === roster.pinned_agent_id);

export async function readProjectAgents(deviceId, entityId) {
  if (!entityId) return null;
  const [support, record] = await readCachedMany([projectAgentsSupportAddress(deviceId), projectAgentsAddress(deviceId, entityId)]);
  return support?.value?.projectWorkers && validProjectRoster(record?.value, entityId) ? record.value : null;
}

export function projectRosterProjection(projects, support, rosterOf) {
  if (!support?.projectWorkers) return projects;
  return projects.map((project) => {
    const entityId = project.entity_id || project.run_id;
    const roster = rosterOf(entityId);
    const projectRoster = validProjectRoster(roster, entityId) ? roster : null;
    return { ...project, projectRoster, assignProjectAgent: support.assignProjectAgent === true };
  });
}

// A new invalidation supersedes a list already in flight, even before it writes.
const reads = new Map();
const SYNC_DEFAULTS = { active: () => true, afterWrite: async () => {} };
const workerRosterAvailable = (capabilities) => (typeof capabilities === "function" ? capabilities() : capabilities)?.agents?.projectWorkers === true;
export async function syncProjectAgents(options) {
  const { deviceId, entityId, capabilities, call, active, afterWrite } = { ...SYNC_DEFAULTS, ...options };
  if (!entityId || !active() || !workerRosterAvailable(capabilities)) return;
  const address = projectAgentsAddress(deviceId, entityId);
  const key = JSON.stringify([deviceId, entityId]);
  const token = {};
  reads.set(key, token);
  const seen = await captureCachedRecord(address);
  if (!active() || !workerRosterAvailable(capabilities)) return;
  const roster = await call("agent.list", { entity_id: entityId, include_project_workers: true });
  if (!validProjectRoster(roster, entityId)) return;
  const generations = await captureRosterGenerations(deviceId, entityId, roster);
  if (!generations.current) return;
  // A reset can replace a thread without touching an as-yet empty roster.
  // Admit the roster only while every generation record is still the one read.
  const written = await mergeCachedRecordsTogether([address, ...generations.addresses], ([held, ...threads]) =>
    active() && reads.get(key) === token && isCachedWrite(held, cachedWriteOf(seen))
      && generationsUnchanged(threads, generations.records) ? [roster] : null);
  if (written) await afterWrite(roster);
}

const generationsUnchanged = (held, seen) => held.every((record, index) => recordWriteOf(record) === recordWriteOf(seen[index]));

const digestNotRetired = (agent, thread) => !thread?.retired_thread_ids?.includes(agent.thread_id)
  && Number(thread?.thread_generation_revision || 0) <= Number(agent.thread_generation_revision || 0);

async function captureRosterGenerations(deviceId, entityId, roster) {
  const agents = roster.agents.filter((agent) => agent.thread_id);
  const addresses = agents.map((agent) => ({ deviceId, entityId, kind: "thread", sub: agent.conversation_id || agent.id }));
  const records = await readCachedMany(addresses);
  return { addresses, records, current: agents.every((agent, index) => digestNotRetired(agent, records[index]?.value)) };
}

export async function projectsWithAgentRosters(deviceId, projects) {
  const owners = projects.map((project) => project.entity_id || project.run_id);
  const records = await readCachedMany([projectAgentsSupportAddress(deviceId), ...owners.map((entityId) => projectAgentsAddress(deviceId, entityId))]);
  const rosters = new Map(owners.map((entityId, index) => [entityId, records[index + 1]?.value]));
  return projectRosterProjection(projects, records[0]?.value, (entityId) => rosters.get(entityId));
}
