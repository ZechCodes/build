import { pushFence, pushedSince } from "./pushFence.js";
import { cachedWriteOf, captureCachedRecord, isCachedWrite, mergeCachedAtomically, mergeCachedRecordsTogether, readCached, readCachedMany } from "./localCache.js";

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

// Only explicit background refreshes share a lane per owner. User refreshes
// dispatch immediately and fence older background reads without waiting.
const reads = new Map();
const immediateReads = new Map();
let requestOrder = 0;
const SYNC_DEFAULTS = { active: () => true, afterWrite: async () => {} };
const workerRosterAvailable = (capabilities) => (typeof capabilities === "function" ? capabilities() : capabilities)?.agents?.projectWorkers === true;
const readerActive = (options) => options.active() && workerRosterAvailable(options.capabilities);

export async function syncProjectAgents(options) {
  const request = { ...SYNC_DEFAULTS, ...options, fence: pushFence(), order: ++requestOrder };
  if (!request.entityId || !readerActive(request)) return;
  const key = JSON.stringify([request.deviceId, request.entityId]);
  if (!request.background) return readImmediateRoster(key, request);
  const running = reads.get(key);
  if (!running) {
    const group = rosterReadGroup(request);
    void runRosterRead(key, group);
    return group.promise;
  }
  if (!running.dispatched) {
    running.requests.push(request);
    return running.promise;
  }
  if (!running.trailing) running.trailing = rosterReadGroup(request);
  else running.trailing.requests.push(request);
  return running.trailing.promise;
}

function rosterReadGroup(request) {
  const group = { requests: [request] };
  group.promise = new Promise((resolve, reject) => { group.resolve = resolve; group.reject = reject; });
  return group;
}

async function runRosterRead(key, group) {
  reads.set(key, group);
  try {
    const roster = await readBackgroundRoster(key, group);
    if (roster) await Promise.all(group.requests.filter(readerActive).map((subscriber) => subscriber.afterWrite(roster)));
    group.resolve();
  } catch (error) {
    group.reject(error);
  } finally {
    reads.delete(key);
    if (group.trailing) void runRosterRead(key, group.trailing);
  }
}

async function readImmediateRoster(key, request) {
  const token = { order: request.order };
  immediateReads.set(key, token);
  token.promise = (async () => {
    const roster = await readRoster({ ...request, active: () => readerActive(request) && immediateReads.get(key) === token });
    token.written = Boolean(roster);
    if (roster && readerActive(request)) await request.afterWrite(roster);
  })();
  return token.promise;
}

async function settledImmediateRead(key) {
  let token;
  do {
    token = immediateReads.get(key);
    await token?.promise?.catch(() => {});
  } while (token !== immediateReads.get(key));
  return token;
}

async function readBackgroundRoster(key, group) {
  while (group.requests.some(readerActive)) {
    const token = await settledImmediateRead(key);
    const request = group.requests.findLast(readerActive);
    if (!request) return;
    group.dispatched = true;
    // A later immediate read already covered these earlier invalidations.
    if (token?.written && request.order < token.order) return (await readCached(projectAgentsAddress(request.deviceId, request.entityId)))?.value;
    const current = () => readerActive(request) && immediateReads.get(key) === token;
    try {
      const roster = await readRoster({ ...request, active: current });
      if (current()) return roster;
    } catch (error) {
      if (current()) throw error;
    }
    // Never admit a retired caller's reply for another subscriber. Retry from
    // a live caller after any immediate read; each invocation keeps its fence.
  }
}

async function readRoster({ deviceId, entityId, capabilities, call, active, fence }) {
  const address = projectAgentsAddress(deviceId, entityId);
  const seen = await captureCachedRecord(address);
  if (!active() || !workerRosterAvailable(capabilities)) return;
  const roster = await call("agent.list", { entity_id: entityId, include_project_workers: true });
  if (!validProjectRoster(roster, entityId)) return;
  const generations = await captureRosterGenerations(deviceId, entityId, roster);
  if (!generations.current) return;
  // A reset can replace a thread without touching an as-yet empty roster.
  // Check generation identity atomically with admission. Message appends may
  // change thread write stamps without changing reset or retirement evidence.
  const written = await mergeCachedRecordsTogether([address, ...generations.addresses], ([held, ...threads]) =>
    active() && workerRosterAvailable(capabilities) && !pushedSince(address, fence) && isCachedWrite(held, cachedWriteOf(seen))
      && generationsUnchanged(threads, generations.records) ? [roster] : null);
  return written ? roster : null;
}

const threadGeneration = (record) => record ? JSON.stringify([
  record.value?.thread_id || "", Number(record.value?.thread_generation_revision || 0), record.value?.retired_thread_ids || [],
]) : null;
const generationsUnchanged = (held, seen) => held.every((record, index) => threadGeneration(record) === threadGeneration(seen[index]));

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
