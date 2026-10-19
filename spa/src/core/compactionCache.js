// A compaction push contains only invalidation. Read authority into the same
// records held by settings and rails, never infer a live observation from a save.
import { entityIdOf } from "./entityId.js";
import { trailingRead } from "./trailingRead.js";
import { notePush } from "./pushFence.js";
import { cachedAddresses, captureCachedRecord, mergeCachedRecordsTogether, readCachedMany, recordWriteOf } from "./localCache.js";

const scopes = new Set(["device", "project", "workspace", "conversation"]);
const kinds = new Set(["settings", "project-settings", "workspace-settings", "row", "project-agents", "feed", "projects", "device-projects", "workspaces"]);
const observationFields = ["compaction", "max_context_tokens", "compact_at_tokens"];
const settingsFields = ["compaction", "compaction_thresholds"];
const reads = new Map();
const invalidations = new Map();
const fieldsOf = (value, fields) => Object.fromEntries(fields.filter((field) => Object.hasOwn(value, field)).map((field) => [field, value[field]]));
const validEvent = (event) => scopes.has(event?.scope) && (event.scope === "device" ? event.owner_id === null : typeof event.owner_id === "string" && event.owner_id.length > 0);
const rowsOf = (value) => Array.isArray(value) ? value : value?.items || [];
const ownerOf = entityIdOf;

function addJob(jobs, method, params, address, rewrite) {
  const key = JSON.stringify([method, params]);
  if (!jobs.has(key)) jobs.set(key, { method, params, targets: [] });
  jobs.get(key).targets.push({ address, rewrite });
}

function replaceAgents(row, answer) {
  if (!Array.isArray(answer?.agents) || !Array.isArray(row?.agents)) return row;
  const agents = new Map(answer.agents.map((agent) => [agent.id, agent]));
  return { ...row, agents: row.agents.map((held) => {
    const fresh = agents.get(held.id);
    // A policy pull cannot restore a conversation generation retired elsewhere.
    if (!fresh || (held.thread_id && fresh.thread_id !== held.thread_id)) return held;
    return { ...held, max_context_tokens: fresh.max_context_tokens ?? null, ...fieldsOf(fresh, observationFields) };
  }) };
}

function agentJobs(jobs, address, value, capabilities) {
  const rows = ["row", "project-agents"].includes(address.kind) ? [value] : rowsOf(value);
  for (const row of rows.filter((row) => Array.isArray(row?.agents))) {
    const entityId = ["row", "project-agents"].includes(address.kind) ? address.entityId : ownerOf(row);
    if (!entityId) continue;
    const params = { entity_id: entityId, ...(capabilities.agents?.projectWorkers ? { include_project_workers: true } : {}) };
    addJob(jobs, "agent.list", params, address, (held, answer) => {
      if (answer?.entity_id && answer.entity_id !== entityId) return null;
      if (["row", "project-agents"].includes(address.kind)) return replaceAgents(held, answer);
      const items = rowsOf(held).map((item) => ownerOf(item) === entityId ? replaceAgents(item, answer) : item);
      return Array.isArray(held) ? items : { ...held, items };
    });
  }
}

function projectFields(row, projects) {
  const fresh = projects.find((project) => project.project_id === (row.project_id || row.id));
  return fresh ? { ...row, ...fieldsOf(fresh, settingsFields) } : row;
}

function projectJobs(jobs, address) {
  if (!["project-settings", "projects", "device-projects", "feed"].includes(address.kind)) return;
  addJob(jobs, "project.list", {}, address, (held, answer) => {
    if (!Array.isArray(answer?.projects)) return null;
    if (address.kind === "project-settings") return answer.projects.find((row) => row.project_id === address.entityId) || null;
    if (Array.isArray(held)) return held.map((row) => projectFields(row, answer.projects));
    return { ...held, projects: (held?.projects || []).map((row) => projectFields(row, answer.projects)) };
  });
}

function settingsJobs(jobs, address) {
  if (address.kind === "settings") addJob(jobs, "settings.get", {}, address, (_held, answer) => answer);
  if (address.kind === "workspace-settings") addJob(jobs, "workspace.get", { workspace_id: address.entityId }, address,
    (_held, answer) => answer?.workspace_id === address.entityId ? answer : null);
  projectJobs(jobs, address);
  if (!["workspaces", "feed"].includes(address.kind)) return;
  addJob(jobs, "workspace.list", {}, address, (held, answer) => {
    if (!Array.isArray(answer?.workspaces)) return null;
    const replace = (row) => {
      const fresh = answer.workspaces.find((item) => (item.workspace_id || item.id) === (row.workspace_id || row.id));
      return fresh ? { ...row, ...fieldsOf(fresh, settingsFields) } : row;
    };
    return Array.isArray(held) ? held.map(replace) : { ...held, workspaces: (held?.workspaces || []).map(replace) };
  });
}

async function refreshJob(context, job) {
  const addresses = job.targets.map((target) => target.address);
  const key = JSON.stringify([context.deviceId, job.method, job.params]);
  const token = {};
  reads.set(key, token);
  const before = await Promise.all(addresses.map(captureCachedRecord));
  if (!context.active()) return;
  const answer = await context.call(job.method, job.params);
  if (!answer || typeof answer !== "object") return;
  await mergeCachedRecordsTogether(addresses, (held) => {
    if (!context.active() || reads.get(key) !== token) return null;
    return held.map((record, index) => recordWriteOf(record) === recordWriteOf(before[index])
      ? job.targets[index].rewrite(record?.value, answer) : null);
  });
}

const scopedAddress = (deviceId, event) => {
  const kind = { device: "settings", project: "project-settings", workspace: "workspace-settings" }[event.scope];
  return kind ? { deviceId, entityId: event.owner_id || "", kind } : null;
};

function affectedAddresses(deviceId, held, events) {
  const addresses = new Map(held.filter((address) => kinds.has(address.kind)).map((address) => [JSON.stringify(address), address]));
  for (const event of events) {
    const address = scopedAddress(deviceId, event);
    if (address && !held.some((row) => row.entityId === address.entityId && row.kind === address.kind)) addresses.set(JSON.stringify(address), address);
  }
  return [...addresses.values()];
}

function markDependencies(address, value) {
  notePush(address);
  if (address.kind !== "feed") return;
  for (const kind of ["projects", "workspaces"]) notePush({ deviceId: address.deviceId, entityId: "", kind });
  for (const row of rowsOf(value)) {
    const entityId = ownerOf(row);
    if (entityId) notePush({ deviceId: address.deviceId, entityId, kind: "row" });
  }
}

const canRefresh = (context, events) => context?.active() && context.capabilities?.compaction?.harnessThresholds && events.length;

/** Ownership and inheritance may have changed since the held lists were read.
 * Refresh all held compaction dependencies on this device rather than guess at
 * descendants from a partial roster. Read jobs run serially because a feed
 * record can hold both settings and observations. The invalidated scope is
 * fetched even when its settings record is cold. */
export async function refreshCompactionCaches(context, event) {
  const events = (Array.isArray(event) ? event : [event]).filter(validEvent);
  if (!canRefresh(context, events)) return;
  const token = {};
  invalidations.set(context.deviceId, token);
  const original = context;
  context = { ...original, active: () => original.active() && invalidations.get(original.deviceId) === token };
  const addresses = affectedAddresses(context.deviceId, await cachedAddresses({ deviceId: context.deviceId }), events);
  const records = await readCachedMany(addresses);
  const jobs = new Map();
  addresses.forEach((address, index) => {
    markDependencies(address, records[index]?.value);
    settingsJobs(jobs, address);
    agentJobs(jobs, address, records[index]?.value, context.capabilities);
  });
  for (const job of jobs.values()) {
    if (!context.active()) return;
    try { await refreshJob(context, job); } catch { /* The next invalidation or greeting retries. */ }
  }
}

// Bursts share one running pass and one trailing pass on the same session.
// A replacement session never waits behind an unreachable older connection.
const pending = new Map();
const refreshPending = trailingRead((deviceId) => {
  const held = pending.get(deviceId);
  const events = [...held.events.values()];
  held.events.clear();
  return refreshCompactionCaches(held.context, events);
}, { generationOf: (deviceId) => pending.get(deviceId)?.context.call });
export function invalidateCompactionCaches(context, event) {
  if (!validEvent(event)) return Promise.resolve();
  const held = pending.get(context.deviceId);
  const events = held?.context.call === context.call ? held.events : new Map();
  events.set(JSON.stringify([event.scope, event.owner_id]), event);
  pending.set(context.deviceId, { context, events });
  return refreshPending(context.deviceId);
}
