// Bounded authoritative project/agent totals and exact project-number lookups.
// Reads paint only committed cache results. Every refresh dispatches once;
// failures retain those results and never imply zero totals or a missing task.
import { mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { nextTaskRead } from "./taskReadOrder.js";

export const TASK_COUNTS_KIND = "tracker-task-counts";
export const TASK_LOOKUP_KIND = "tracker-task-lookup";
const encoder = new TextEncoder();
const bytes = (value) => encoder.encode(value).length;
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const exactId = (value) => typeof value === "string" && value.length > 0 && bytes(value) <= 128;
const natural = (value) => Number.isSafeInteger(value) && value >= 0;
const address = ({ deviceId, projectId }, kind, sub) => ({ deviceId, entityId: String(projectId), kind, sub });

function canonicalAgents(agentIds = []) {
  if (!Array.isArray(agentIds) || agentIds.length > 100 || !agentIds.every(exactId)) {
    throw new Error("Expected at most 100 exact task-count agent IDs");
  }
  return [...new Set(agentIds)].sort();
}

function taskNumber(number) {
  // Larger JSON numbers cannot identify an exact integer in this client.
  if (!Number.isSafeInteger(number) || number <= 0) throw new Error("Expected an exact positive task number");
  return number;
}

export const taskCountsAddress = (scope, agentIds = []) => address(scope, TASK_COUNTS_KIND, JSON.stringify(canonicalAgents(agentIds)));
export const taskLookupAddress = (scope, number) => address(scope, TASK_LOOKUP_KIND, String(taskNumber(number)));

export async function readTaskCounts(scope, agentIds = []) {
  return (await readCached(taskCountsAddress(scope, agentIds)))?.value?.result ?? null;
}

const unknownLookup = (scope, number) => ({ status: "unknown", project_id: scope.projectId, number });
export async function readTaskLookup(scope, number) {
  return (await readCached(taskLookupAddress(scope, number)))?.value?.result ?? unknownLookup(scope, number);
}

function validateEnvelope(scope, result) {
  if (!object(result) || result.project_id !== scope.projectId || typeof result.basis !== "string" || !result.basis) {
    throw new Error("Expected matching task query result and opaque basis");
  }
  if (bytes(JSON.stringify(result)) > 65_536) throw new Error("Task query result exceeds its byte bound");
}

function validateCounts(scope, agentIds, result) {
  validateEnvelope(scope, result);
  validateBucket(result);
  if (!Array.isArray(result.agents) || result.agents.length !== agentIds.length) {
    throw new Error("Expected exactly the requested task-count agent buckets");
  }
  const requested = new Set(agentIds);
  for (const bucket of result.agents) {
    validateBucket(bucket);
    if (!requested.delete(bucket.agent_id)) throw new Error("Unexpected or duplicate task-count agent bucket");
  }
  return result;
}

function validateBucket(bucket) {
  if (!natural(bucket?.unfinished_open_count) || !natural(bucket?.watched_unread_total)) {
    throw new Error("Expected exact nonnegative task counts");
  }
}

function validateLookup(scope, number, result) {
  validateEnvelope(scope, result);
  if (result.number !== number || !object(result.identities)) throw new Error("Expected matching task lookup and identities");
  if (!Object.entries(result.identities).every(([id, identity]) => exactId(id) && object(identity))) {
    throw new Error("Invalid task lookup identity");
  }
  if (result.task === null) return { ...result, status: "missing" };
  validateLookupTask(scope, number, result.task);
  // Retain precisely the summary and identities. Detail/timeline normalization
  // belongs to its existing owner and is never fabricated from this result.
  return { ...result, status: "found" };
}

function validateLookupTask(scope, number, task) {
  if (!object(task) || typeof task.id !== "string" || !task.id || task.number !== number || task.project_id !== scope.projectId) {
    throw new Error("Expected a matching bounded task summary");
  }
}

function reserve(address, generation, accept) {
  return mergeCachedRecordsTogether([address], ([record]) => {
    const held = record?.value;
    if (!accept() || held?.generation >= generation) return null;
    return [{ generation, result: held?.result ?? null }];
  });
}

function publish(address, generation, result, accept) {
  return mergeCachedRecordsTogether([address], ([record]) => {
    if (!accept() || record?.value?.generation !== generation) return null;
    return [{ generation, result }];
  });
}

/** The owner supplies live greeting/session/account authority. active() must
 * also deny dispatch while a local mutation is pending; mutationEpoch() changes
 * at pending start and acknowledgment, fencing reads across either transition.
 * A write acknowledgment is never interpreted as a query basis. */
function createPointReader({ scope: suppliedScope, callRpc, capabilities, sessionOf,
  active = () => true, mutationEpoch = () => null }, contract) {
  const scope = { ...suppliedScope };
  let disposed = false;
  const available = () => !disposed && active() && capabilities()?.[contract.capability] === true;
  const read = (selection) => contract.read(scope, selection);

  async function refresh(selection) {
    const selected = contract.select(selection);
    if (!available()) return false;
    const session = sessionOf(), epoch = mutationEpoch();
    const accept = () => available() && sessionOf() === session && mutationEpoch() === epoch;
    const generation = await nextTaskRead();
    const recordAddress = contract.address(scope, selected);
    if (!await reserve(recordAddress, generation, accept) || !accept()) return false;
    const response = await callRpc(contract.method, contract.params(scope, selected));
    if (!accept()) return false;
    const result = contract.validate(scope, selected, response);
    return publish(recordAddress, generation, result, accept);
  }

  return { read, refresh, dispose() { disposed = true; } };
}

export function createTaskAggregateReader(options) {
  return createPointReader(options, {
    capability: "counts", method: "tasks.counts", select: canonicalAgents,
    address: taskCountsAddress, read: readTaskCounts, validate: validateCounts,
    params: (scope, agentIds) => ({ project_id: scope.projectId, agent_ids: agentIds }),
  });
}

export function createTaskLookupReader(options) {
  return createPointReader(options, {
    capability: "lookup", method: "tasks.lookup", select: taskNumber,
    address: taskLookupAddress, read: readTaskLookup, validate: validateLookup,
    params: (scope, number) => ({ project_id: scope.projectId, number }),
  });
}
