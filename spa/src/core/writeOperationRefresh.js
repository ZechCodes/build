import { cachedAddresses, readCanonicalCached, recordWriteOf } from "./localCache.js";
import { captureTaskDetailRead, taskAddress, TRACKER_TASK_KIND } from "./trackerCache.js";
import { captureTaskContentRead } from "./taskContentAuthority.js";
import { threadCacheAddress } from "./conversationCache.js";
import { threadWindow } from "./threadSync.js";
import { recordKey } from "./idbRecords.js";

const paramsOf = (record) => record.request?.params || {};
const refreshReads = new WeakMap();
const addressKey = (address) => JSON.stringify([address.deviceId, address.entityId, address.kind, address.sub || ""]);
const refreshError = () => new Error("The required canonical refresh cannot be mapped to a known record");
const invalidTaskRefresh = (message) => Object.assign(new Error(message), { error_code: "invalid-task-response" });
const assertActive = (authority) => {
  if (!authority?.active()) throw new Error("The canonical refresh opening ended");
};

const threadParams = (record) => {
  const params = paramsOf(record);
  const names = ["entity_id", "agent_id", "conversation_id", "thread_id", "thread_generation_revision"];
  return { ...Object.fromEntries(names.filter((name) => params[name] !== undefined).map((name) => [name, params[name]])), newest: true, limit: 100 };
};

function assertThreadGeneration(params, page) {
  if (params.thread_id && page.thread_id !== params.thread_id) throw new Error("Conversation generation changed during refresh");
  if (params.thread_generation_revision !== undefined && page.thread_generation_revision !== params.thread_generation_revision) {
    throw new Error("Conversation generation changed during refresh");
  }
}

const READERS = {
  "tracker-task": async (address, _record, authority) => {
    const result = await authority.call("tasks.get", { task_id: address.sub });
    if (!result?.task || !Array.isArray(result.timeline)) throw invalidTaskRefresh("Task refresh is incomplete");
    if (result.task.id !== address.sub || result.task.project_id !== address.entityId) throw invalidTaskRefresh("Task refresh identity changed");
    return { task: result.task, timeline: result.timeline };
  },
  thread: async (_address, record, authority, previous) => {
    const params = threadParams(record);
    const page = await authority.call("thread.page", params);
    if (!Array.isArray(page?.items)) throw new Error("Conversation refresh is incomplete");
    assertThreadGeneration(params, page);
    const window = threadWindow(previous?.value, page, { newest: true });
    if (!window) throw new Error("Conversation generation changed during refresh");
    return window;
  },
};

function explicitProject(frame, record) {
  return record.owner?.projectId || paramsOf(record).project_id || frame.result?.task?.project_id;
}

function publicationProject(frame, record) {
  const owners = [...(frame.published || []), ...(record.owners || [])];
  const projects = [...new Set(owners.filter((owner) => owner.kind === "project").map((owner) => owner.id))];
  return projects.length === 1 ? projects[0] : null;
}

async function taskRefreshAddress(item, frame, record) {
  const project = explicitProject(frame, record) || publicationProject(frame, record);
  if (project) return taskAddress(record.deviceId, project, item.id);
  const held = await cachedAddresses({ deviceId: record.deviceId });
  const matches = held.filter((address) => address.deviceId === record.deviceId && address.kind === TRACKER_TASK_KIND && address.sub === item.id);
  if (matches.length !== 1) throw refreshError();
  return matches[0];
}

function threadRefreshAddress(item, _frame, record) {
  const params = paramsOf(record);
  const identities = [params.thread_id, params.conversation_id, params.agent_id].filter(Boolean);
  if (!params.entity_id || !identities.includes(item.id)) throw refreshError();
  return threadCacheAddress({ deviceId: record.deviceId, entityId: params.entity_id,
    agentId: params.agent_id, conversationId: params.conversation_id, threadId: params.thread_id });
}

const REFRESH_ADDRESSES = { task: taskRefreshAddress, thread: threadRefreshAddress };

async function requiredAddress(item, frame, record) {
  if (!record.deviceId || !item?.id || !Object.hasOwn(REFRESH_ADDRESSES, item.kind)) throw refreshError();
  return REFRESH_ADDRESSES[item.kind](item, frame, record);
}

async function refreshAddresses(frame, record) {
  const addresses = new Map();
  for (const item of frame.refresh || []) {
    const address = await requiredAddress(item, frame, record);
    addresses.set(addressKey(address), address);
  }
  for (const { address } of record.overlay || []) {
    if (address?.deviceId !== record.deviceId || !Object.hasOwn(READERS, address.kind)) continue;
    addresses.set(addressKey(address), address);
  }
  return [...addresses.values()];
}

async function publicationGuard(address, previous) {
  const guard = { written: recordWriteOf(previous) };
  if (address.kind === TRACKER_TASK_KIND) guard.detailBasis = await captureTaskDetailRead(address.deviceId, address.entityId, address.sub);
  return guard;
}

async function refreshAddress(address, { cache, frame, record, authority, refresh }) {
  const active = authority.active;
  const previous = await readCanonicalCached(address);
  const guard = await publicationGuard(address, previous);
  const contentRead = address.kind === TRACKER_TASK_KIND
    ? await captureTaskContentRead(address, { revalidate: "full" }) : undefined;
  assertActive(authority);
  const value = await READERS[address.kind](address, record, authority, previous);
  assertActive(authority);
  const task = address.kind === TRACKER_TASK_KIND;
  const entry = publicationEntry({ address, value, publication: frame.published || [], guard,
    ...(contentRead ? { contentRead } : {}) }, active);
  const result = await cache.observePublication({
    accept: active,
    ...(refresh ? { refresh } : {}),
    ...(task || refresh ? { report: true } : {}),
    canonical: [entry],
  });
  assertActive(authority);
  if (task || refresh) return result;
  const written = result;
  if (!written?.some((target) => addressKey(target) === addressKey(address))) {
    throw new Error("The canonical publication was superseded during refresh");
  }
}

function freezeTree(value) {
  if (!value || typeof value !== "object") return value;
  for (const child of Object.values(value)) freezeTree(child);
  return Object.freeze(value);
}

function publicationEntry(input, active) {
  const entry = Object.freeze({ ...input, address: Object.freeze({ ...input.address }),
    value: freezeTree(structuredClone(input.value)), guard: Object.freeze({ ...input.guard }),
    publication: freezeTree(structuredClone(input.publication)),
    contentRead: freezeTree(input.contentRead) });
  refreshReads.set(entry, Object.freeze({ value: entry.value, active }));
  return entry;
}

// Registration belongs only to the actual literal read owners above, after
// their identity/generation checks. A permit alone conveys no read evidence.
export function refreshReadOf(entry) {
  return refreshReads.get(entry) || null;
}

// A thread read never confers task-history authority.
export function taskHistoryReadOf(entry) {
  return entry.address.kind === TRACKER_TASK_KIND ? refreshReadOf(entry) : null;
}

async function taskRefreshOpening(targets, { cache, frame, record, authority }) {
  const opening = cache.openRefresh ? await cache.openRefresh({ operationId: record.operationId,
    revision: frame.revision, targets, accept: authority.active }) : { state: "unbound" };
  assertActive(authority);
  const saved = await cache.get(record.operationId);
  const refresh = { operationId: record.operationId, revision: frame.revision,
    generation: saved?.refreshState?.generation, state: opening.state };
  if (saved && saved.revision !== frame.revision) refresh.state = "superseded";
  const unread = new Set((saved?.refreshState?.targets || []).filter(({ phase }) => phase === "unread").map(({ key }) => key));
  return { attempt: opening.attempt, refresh, targets: readableTaskTargets(targets, refresh.state, unread, !saved) };
}

function readableTaskTargets(targets, state, unread, detached) {
  if (state === "unbound") return detached ? targets : [];
  return state === "ready" ? targets.filter((address) => unread.has(recordKey(address))) : [];
}

async function refreshTaskTarget(address, input) {
  try { return await refreshAddress(address, input); }
  catch (error) {
    if (!input.refresh || error.error_code !== "invalid-task-response") throw error;
    return input.cache.observePublication({ refresh: input.refresh, report: true, accept: input.authority.active,
      failure: { targetKey: recordKey(address), reason: "invalid-task-response" } });
  }
}

async function refreshTaskTargets(targets, input) {
  const opening = await taskRefreshOpening(targets, input);
  let refresh = opening.refresh;
  const outcomes = [];
  for (const address of opening.targets) {
    const result = await refreshTaskTarget(address, { ...input, refresh: opening.attempt });
    outcomes.push(...(result?.outcomes || []));
    if (result?.refresh) refresh = result.refresh;
    if (["superseded", "inactive"].includes(refresh.state)) break;
  }
  return { complete: refresh.state === "complete", outcomes, refresh };
}

/** Resolve closed domain invalidations independently of accepted overlays.
 * Every required read must land under the captured opening and cache fences
 * before the persistent service can clear its refresh obligation. */
export async function refreshWriteOperation(input) {
  const { cache, frame, record, authority } = input;
  assertActive(authority);
  const addresses = await refreshAddresses(frame, record);
  if (addresses.some((address) => address.kind === TRACKER_TASK_KIND)) return refreshTaskTargets(addresses, input);
  for (const address of addresses) await refreshAddress(address, input);
  if (frame.state === "succeeded" && (await cache.get(record.operationId))?.overlayActive) {
    throw new Error("The canonical publication has not reached the cache yet");
  }
}
