// Client observation provenance for ordinary task bytes. These clocks never
// describe server revisions, timeline completeness, or opaque projection bases.
import { mergeCachedRecordsTogether } from "./localCache.js";
import { randomToken } from "./idbRecords.js";
import { taskMutationsPending, taskMutationsPendingFor } from "./taskMutationFence.js";
import { nextTaskRead, nextTaskReadTicket, TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress, taskWatchControlAddress, initialTaskWatchControl,
  taskWatchIntentAddress } from "./taskReadOrder.js";
export { TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress, taskWatchControlAddress };
export const TASK_CONTENT_CACHE = "__task_content_cache";
const OBSERVATION = "__task_content_observation";
const headerFields = `id project_id number title state status labels priority assignee created_by created_at updated_at
closed_at done_at trackers attachment_count summary_truncated deferred_assignment`.split(/\s+/);
const bodyFields = ["body", "attachments", "identities"];
const nested = {
  header: { links: ["workspace_ids", "parent_task_id"], dependencies: ["blocked", "waiting_on_count", "blocks_count"] },
  body: { links: ["branches", "commits", "conversation_ids"], dependencies: ["waiting_on", "blocks", "blocks_next_cursor"] },
};
const fields = { header: headerFields, body: bodyFields };
const sameAddress = (a, b) => a && b && ["deviceId", "entityId", "kind", "sub"].every((key) => (a[key] ?? "") === (b[key] ?? ""));
const scopeOf = (place) => ({ deviceId: place.deviceId, projectId: place.projectId ?? place.entityId });
const cacheOf = (task) => task?.[TASK_CONTENT_CACHE] || {};
const validScope = (scope) => typeof scope.deviceId === "string" && typeof scope.projectId === "string";

export async function captureTaskContentRead(place, { revalidate = false } = {}) {
  const { scope, address } = capturePlace(place, revalidate);
  const localMutation = captureLocalTaskMutation(scope, address);
  const allocation = await nextTaskReadTicket(scope, { address });
  const readOrder = allocation?.readOrder ?? await nextTaskRead();
  const read = capturedRead(allocation, address, revalidate, readOrder);
  if (localMutation) read.localOrdinaryMutation = { ...localMutation,
    pending: localMutation.pending || taskMutationsPendingFor(scope, localMutation.taskId) > 0 };
  return read;
}

function captureLocalTaskMutation(scope, address) {
  if (address?.kind !== "tracker-task" || !address.sub) return null;
  return { ...scope, taskId: address.sub, pending: taskMutationsPendingFor(scope, address.sub) > 0 };
}

function capturePlace(place, revalidate) {
  const scope = scopeOf(place), address = place.kind ? { ...place } : undefined;
  if (!validScope(scope) || ![false, "summary", "full"].includes(revalidate)) throw new Error("Invalid task content capture");
  if (revalidate === "full" && !address) throw new Error("Full task revalidation needs its canonical address");
  return { scope, address };
}

function capturedRead(allocation, address, revalidate, readOrder) {
  const ticket = allocation && { storeGeneration: allocation.storeGeneration, scopeGeneration: allocation.scopeGeneration,
    retirementEpoch: allocation.retirementEpoch, watchGeneration: allocation.watchGeneration,
    watchRetirementEpoch: allocation.watchRetirementEpoch, watchPendingGeneration: allocation.watchPendingGeneration,
    watchBlindRetirementEpoch: allocation.watchBlindRetirementEpoch, watchPending: allocation.watchPending, watchUncertain: allocation.watchUncertain };
  return { readOrder, ticket, revalidate, ...(address ? { address, written: allocation?.written } : {}) };
}

export const isObservedTaskContent = (task) => Boolean(task?.[OBSERVATION]);
export const taskContentObservationOf = (task) => task?.[OBSERVATION];

export function observeTaskContent(raw, { contentRead, coverage } = {}) {
  if (!raw?.id || !["summary", "full"].includes(coverage)) throw new Error("Invalid task content observation");
  const task = { ...raw }; delete task[OBSERVATION];
  task[OBSERVATION] = { contentRead, coverage };
  return task;
}

export function retireTaskContent(task, component = "full") {
  if (!["header", "body", "full"].includes(component)) throw new Error("Invalid task content retirement");
  const next = { ...task }, metadata = { ...cacheOf(task) }, retired = { ...metadata.retired };
  for (const name of component === "full" ? ["header", "body"] : [component]) {
    metadata.timestamps = { ...metadata.timestamps, [name]: metadata[name]?.updated_at ?? metadata.timestamps?.[name] ?? task.updated_at };
    delete metadata[name]; retired[name] = randomToken();
  }
  next[TASK_CONTENT_CACHE] = { ...metadata, retired };
  delete next[OBSERVATION];
  return next;
}

export function taskContentFragment(scope, { watchTaskIds = [] } = {}) {
  const ids = distinctWatchTaskIds(watchTaskIds);
  return {
    addresses: [TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress(scope), taskWatchControlAddress(scope),
      ...ids.map((id) => taskWatchIntentAddress(scope, id))],
    context: (records) => contentWatchContext(scope, records, ids),
    finish: ([store, domain, control], { retire = false, retireWatch = false } = {}) => {
      if (!retire && !retireWatch) return Array(3 + ids.length).fill(null);
      return [store ? null : { generation: randomToken() }, retire ? retirementDomain(domain?.value) : null,
        retireWatch ? retiredWatchControl(control) : null, ...ids.map(() => null)];
    },
  };
}

function distinctWatchTaskIds(ids) {
  if (!Array.isArray(ids) || ids.some((id) => typeof id !== "string" || !id)) throw new Error("Invalid admitted watch intent identities");
  return [...new Set(ids)];
}

function contentWatchContext(scope, records, ids) {
  const notes = new Map(ids.map((id, index) => [id, records[index + 3]?.value]));
  return { ...contentContext(records), localWatchPending: taskMutationsPending(scope.deviceId) > 0,
    watchIntentOf: (id) => notes.get(id) };
}

export function withTaskWatchIntentNote(context, taskId, note) {
  return { ...context, watchIntentOf: (id) => id === taskId ? note : context.watchIntentOf?.(id) };
}

const retiredWatchControl = (record) => ({ ...(record?.value || initialTaskWatchControl()),
  uncertain: record?.value?.uncertain !== false || !record?.value?.blindRetirementEpoch,
  watchRetirementEpoch: randomToken(), blindRetirementEpoch: randomToken() });

function contentContext([store, domain, control]) {
  const held = domain?.value || {}, watch = control?.value || {};
  return { storeGeneration: store?.value?.generation, scopeGeneration: held.generation, retirementEpoch: held.retirementEpoch,
    watchGeneration: watch.generation, watchRetirementEpoch: watch.watchRetirementEpoch,
    watchPendingGeneration: watch.pendingGeneration,
    watchPendingCount: watch.pendingCount ?? null, watchUncertain: watch.uncertain !== false || !watch.blindRetirementEpoch };
}

function retirementDomain(held) {
  return { ...held, generation: held?.generation || randomToken(), retirementEpoch: randomToken() };
}

export function mergeTaskContentRecordsTogether(scope, addresses, reducer, options) {
  const fragment = taskContentFragment(scope, options);
  return mergeCachedRecordsTogether([...addresses, ...fragment.addresses], (all) => {
    const shared = all.slice(addresses.length), result = reducer(all.slice(0, addresses.length), fragment.context(shared));
    return result ? [...result.values, ...fragment.finish(shared, { retire: result.retire === true, retireWatch: result.retireWatch === true })] : null;
  });
}

function validObservation(observation, context, address, heldWritten) {
  const read = observation?.contentRead, ticket = read?.ticket;
  if (!Number.isSafeInteger(read?.readOrder) || !ticket || !context) return false;
  if (!["storeGeneration", "scopeGeneration", "retirementEpoch"].every((key) => ticket[key] && ticket[key] === context[key])) return false;
  return !sameAddress(read.address, address) || read.written === heldWritten;
}

/** Whole-record membership requires its own exact addressed capture. The
 * more permissive per-row header rule cannot establish a list's absence. */
export function acceptsTaskContentRead(contentRead, { context, address, heldWritten }) {
  return Boolean(sameAddress(contentRead?.address, address)
    && validObservation({ contentRead }, context, address, heldWritten));
}

const sameLifetime = (metadata, context) => metadata.storeGeneration === context?.storeGeneration
  && metadata.scopeGeneration === context?.scopeGeneration;
const proofOf = (task, component, context) => sameLifetime(cacheOf(task), context) ? cacheOf(task)[component] : undefined;
const newerTimestamp = (proof, task) => Date.parse(proof?.updated_at) > Date.parse(task?.updated_at);
const covered = (observation, component) => component === "header" || observation?.coverage === "full";
const ownFullBaseline = (observation, options) => observation.contentRead.revalidate === "full"
  && sameAddress(observation.contentRead.address, options.address);
const revalidates = (observation, component, options) => component === "header"
  ? ["summary", "full"].includes(observation.contentRead.revalidate) : ownFullBaseline(observation, options);
const componentTimestamp = (task, component, own) => own?.updated_at ?? cacheOf(task).timestamps?.[component] ?? task?.updated_at;

function blockedComponent(own, barrier, incoming, read, heldTimestamp) {
  return own?.read_order > read || barrier?.read_order > read || newerTimestamp(barrier, incoming)
    || Date.parse(heldTimestamp) > Date.parse(incoming.updated_at);
}

function guardedHeader(guard, incoming, read, context) {
  const proof = proofOf(guard, "header", context);
  return proof?.read_order > read || Date.parse(componentTimestamp(guard, "header", proof)) > Date.parse(incoming.updated_at);
}

function otherHeaderBlocks(options, incoming, read) {
  return (options.headerGuards || []).some((guard) => guard.id === incoming.id && guardedHeader(guard, incoming, read, options.context));
}

/** One real paint baseline; independent constraints remain guard-only. */
export function taskContentHeaderBaseline(tasks, context) {
  const unknown = tasks.find((task) => !proofOf(task, "header", context));
  if (unknown) return unknown;
  return tasks.reduce((held, task) => !held || proofOf(task, "header", context).read_order > proofOf(held, "header", context).read_order ? task : held, undefined);
}

function componentWinner(held, incoming, component, observation, options) {
  if (!covered(observation, component) || !validObservation(observation, options.context, options.address, options.heldWritten)) return false;
  if (component === "body" && typeof incoming.body !== "string") return false;
  const own = proofOf(held, component, options.context), barrier = proofOf(options.headerBarrier, "header", options.context);
  const ownHeader = component === "body" ? proofOf(held, "header", options.context) : undefined;
  const read = observation.contentRead.readOrder;
  const heldTimestamp = componentTimestamp(held, component, own);
  if (otherHeaderBlocks(options, incoming, read)) return false;
  if (Date.parse(componentTimestamp(options.headerBarrier, "header", barrier)) > Date.parse(incoming.updated_at)) return false;
  if (blockedComponent(own, barrier, incoming, read, heldTimestamp)
    || blockedComponent(undefined, ownHeader, incoming, read, undefined)) return false;
  return establishedComponent(held, own, observation, component, options);
}

function establishedComponent(held, own, observation, component, options) {
  // A current domain capture covers IDs discovered by the RPC. Existing
  // unknown components still require their explicit revalidation path.
  return Boolean(own) || component === "header" && !held || revalidates(observation, component, options);
}

function copyFields(target, source, names) {
  for (const key of names) {
    delete target[key];
    if (Object.hasOwn(source || {}, key)) target[key] = source[key];
  }
}

function applyComponent(target, source, component) {
  copyFields(target, source, fields[component]);
  for (const [key, names] of Object.entries(nested[component])) {
    const next = { ...target[key] }; copyFields(next, source?.[key], names);
    if (Object.keys(next).length) target[key] = next;
    else delete target[key];
  }
  return target;
}

export function withTaskContentComponents(target, source, coverage = "full") {
  const task = { ...target }; applyComponent(task, source, "header");
  if (coverage === "full") applyComponent(task, source, "body");
  const metadata = { ...cacheOf(source) };
  if (coverage === "summary") { delete metadata.body; delete metadata.bodyHeaderIdentity; }
  task[TASK_CONTENT_CACHE] = metadata; delete task[OBSERVATION];
  return task;
}

const componentSnapshot = (task, component) => applyComponent({}, task, component);
export const sameTaskContentComponent = (before, after, component) => JSON.stringify(componentSnapshot(before, component))
  === JSON.stringify(componentSnapshot(after, component));

function blindComponent(held, candidate, component, context) {
  const prior = cacheOf(held), next = cacheOf(candidate);
  const explicit = next.retired?.[component] && next.retired[component] !== prior.retired?.[component];
  if (explicit) return true;
  if (held) return !sameTaskContentComponent(held, candidate, component);
  return !proofOf(candidate, component, context);
}

function copiedComponentSource(held, candidate, component, context) {
  if (!held) return candidate;
  const before = proofOf(held, component, context), next = proofOf(candidate, component, context);
  // A cached copy cannot repair an unknown target, even when bytes match.
  if (!before) return held;
  return next?.read_order > before.read_order ? candidate : held;
}

function copyMetadataField(target, source, key) {
  if (Object.hasOwn(source, key)) target[key] = source[key];
  else delete target[key];
}

function copyComponentMetadata(metadata, source, component, context) {
  const cached = cacheOf(source), proof = proofOf(source, component, context);
  if (proof) metadata[component] = proof;
  else delete metadata[component];
  for (const name of ["retired", "identities", "timestamps"]) {
    metadata[name] = { ...metadata[name] };
    copyMetadataField(metadata[name], cached[name] || {}, component);
  }
  if (component === "body") for (const key of ["bodyHeaderIdentity", "coveredHeaderIdentity", "coveredHeaderRetirement"]) {
    copyMetadataField(metadata, cached, key);
  }
  if (proof) Object.assign(metadata, { storeGeneration: context.storeGeneration, scopeGeneration: context.scopeGeneration });
}

function retireWrittenComponent(task, candidate, component) {
  const next = retireTaskContent(task, component), explicit = cacheOf(candidate).retired?.[component];
  if (explicit) next[TASK_CONTENT_CACHE].retired[component] = explicit;
  return next;
}

/** Exact copies select the actual accepted observation separately for each
 * unchanged component. Local retirement never borrows another copy's proof. */
export function reconcileTaskContentWrite(held, candidate, options = {}) {
  if (isObservedTaskContent(candidate)) return reconcileTaskContent(held, candidate, options);
  let task = { ...candidate, [TASK_CONTENT_CACHE]: { ...cacheOf(held || candidate) } }, retired = false;
  const sources = {};
  for (const component of ["header", "body"]) {
    sources[component] = copiedComponentSource(held, candidate, component, options.context);
    copyComponentMetadata(task[TASK_CONTENT_CACHE], sources[component], component, options.context);
    if (!blindComponent(held, candidate, component, options.context)) continue;
    task = retireWrittenComponent(task, candidate, component); retired = true;
  }
  task[TASK_CONTENT_CACHE].needs_full_refresh = Boolean(retired || cacheOf(sources.body).needs_full_refresh || fullRefreshNeeded(task, options.headerBarrier));
  return { task, retired, acceptedHeader: false, acceptedBody: false, needsFullRefresh: taskNeedsFullRefresh(task) };
}

function componentIdentity(held, incoming, component, prior) {
  if (component === "header") return JSON.stringify(componentSnapshot(incoming, component));
  return sameTaskContentComponent(held, incoming, component) && prior || randomToken();
}

function observationMetadata(held, incoming, observation, accepted, context, headerBarrier, address) {
  const metadata = sameLifetime(cacheOf(held), context) ? { ...cacheOf(held) } : { retired: { ...cacheOf(held).retired } };
  delete metadata.barrier;
  const retired = { ...metadata.retired };
  const identities = { ...metadata.identities }, timestamps = { ...metadata.timestamps };
  for (const component of ["header", "body"]) if (accepted[component]) {
    metadata[component] = { read_order: observation.contentRead.readOrder, updated_at: observation.updated_at };
    // Recovery identity describes bytes, not a publication event. It is never
    // used to admit a write: ticket generations and exact baselines do that.
    identities[component] = componentIdentity(held, incoming, component, identities[component]);
    timestamps[component] = observation.updated_at;
    delete retired[component];
  }
  if (accepted.body) {
    metadata.bodyHeaderIdentity = identities.header;
    metadata.coveredHeaderIdentity = cacheOf(headerBarrier).identities?.header;
    metadata.coveredHeaderRetirement = ownFullBaseline(observation, { address })
      ? cacheOf(headerBarrier).retired?.header : undefined;
  }
  return { ...metadata, storeGeneration: context?.storeGeneration, scopeGeneration: context?.scopeGeneration, retired, identities, timestamps };
}

function retainedTask(held, incoming, observation) {
  if (held) return { ...held };
  const task = { ...incoming }; delete task[OBSERVATION]; delete task[TASK_CONTENT_CACHE];
  if (observation?.coverage === "summary") applyComponent(task, undefined, "body");
  return task;
}

function selectedTask(held, incoming, observation, accepted, options) {
  const task = retainedTask(held, incoming, observation);
  if (!held && options.headerBarrier && !accepted.body) {
    applyComponent(task, undefined, "body"); applyComponent(task, options.headerBarrier, "header");
  }
  for (const name of ["header", "body"]) if (accepted[name]) applyComponent(task, incoming, name);
  return task;
}

const mismatchedTask = (held, candidate) => !candidate?.id || held?.id && held.id !== candidate.id;
const staleAbsent = (held, observation, options) => observation && !held
  && !validObservation(observation, options.context, options.address, options.heldWritten);

function capturedTaskPending(read, scope, taskId) {
  const local = read?.localOrdinaryMutation;
  return local?.pending === true && local.deviceId === scope.deviceId
    && local.projectId === scope.projectId && local.taskId === taskId;
}

function pendingRetiredHeader(held, incoming, observation, options) {
  if (!validObservation(observation, options.context, options.address, options.heldWritten)) return false;
  const scope = { deviceId: options.address?.deviceId, projectId: options.address?.entityId };
  const pending = capturedTaskPending(observation.contentRead, scope, incoming.id)
    || taskMutationsPendingFor(scope, incoming.id) > 0;
  if (!pending) return false;
  return [held, options.headerBarrier, ...(options.headerGuards || [])]
    .some((task) => task?.id === incoming.id && typeof cacheOf(task).retired?.header === "string");
}

export function reconcileTaskContent(held, candidate, options = {}) {
  if (mismatchedTask(held, candidate)) throw new Error("Mismatched task content identity");
  const observation = candidate[OBSERVATION];
  if (staleAbsent(held, observation, options)) {
    return { task: undefined, acceptedHeader: false, acceptedBody: false, needsFullRefresh: true, retired: false };
  }
  const incoming = { ...candidate }; delete incoming[OBSERVATION];
  const admittedHeader = componentWinner(held, incoming, "header", observation, options);
  const ordinaryDeferred = admittedHeader && pendingRetiredHeader(held, incoming, observation, options);
  const header = admittedHeader && !ordinaryDeferred;
  const accepted = { header, body: header && componentWinner(held, incoming, "body", observation, options) };
  const task = selectedTask(held, incoming, observation, accepted, options);
  const metadata = observationMetadata(held, incoming, { ...observation, updated_at: incoming.updated_at }, accepted, options.context, options.headerBarrier, options.address);
  task[TASK_CONTENT_CACHE] = metadata;
  const needsFullRefresh = observation?.coverage === "full" && !accepted.body || fullRefreshNeeded(task, options.headerBarrier);
  metadata.needs_full_refresh = needsFullRefresh;
  return { task, acceptedHeader: accepted.header, acceptedBody: accepted.body, needsFullRefresh, retired: false, ordinaryDeferred };
}

const retiredBarrier = (metadata, barrier) => Boolean(barrier.retired?.header
  && metadata.coveredHeaderRetirement !== barrier.retired.header);
const retiredRefresh = (metadata, barrier) => Boolean(metadata.retired?.body || metadata.retired?.header || retiredBarrier(metadata, barrier));
const coveredHeader = (metadata, identity) => metadata.bodyHeaderIdentity === identity || metadata.coveredHeaderIdentity === identity;

function fullRefreshNeeded(task, barrier) {
  const metadata = cacheOf(task), header = cacheOf(barrier).header;
  if (retiredRefresh(metadata, cacheOf(barrier))) return true;
  if (!metadata.body) return Boolean(header);
  if (!header) return Boolean(metadata.header && !coveredHeader(metadata, metadata.identities?.header));
  if (!sameLifetime(metadata, cacheOf(barrier))) return true;
  return !coveredHeader(metadata, cacheOf(barrier).identities?.header);
}

export const taskNeedsFullRefresh = (task, headerBarrier) => Boolean(cacheOf(task).needs_full_refresh || fullRefreshNeeded(task, headerBarrier));
export function taskFullRefreshKey(task, headerBarrier) {
  if (!taskNeedsFullRefresh(task, headerBarrier)) return null;
  const metadata = cacheOf(task), barrier = headerBarrier ? cacheOf(headerBarrier) : metadata.barrier || {};
  return JSON.stringify([task?.id ?? headerBarrier?.id, ...refreshLifetime(metadata), ...refreshLifetime(barrier), ...refreshComponents(metadata, barrier)]);
}

const refreshLifetime = (metadata) => [metadata.storeGeneration || "legacy", metadata.scopeGeneration || "legacy"];
const refreshHeader = (metadata, barrier) => [barrier.identities?.header || metadata.identities?.header || "unknown-header",
  barrier.retired?.header || "known-barrier", metadata.retired?.header || "known-header"];
const refreshComponents = (metadata, barrier) => [metadata.retired?.body || "unknown-body", metadata.identities?.body || "unknown-body",
  ...refreshHeader(metadata, barrier)];
