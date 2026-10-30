// Pure selection of independently observed task families. The caller supplies
// the current shared control in its publication transaction. No receipt,
// ordinary-content timestamp, membership clock or equal snapshot is freshness.
import { compareTaskMarkers, newerTaskMarker } from "./taskOpaqueMarkers.js";
import { latestTaskMark, taskUnreadKey } from "./trackerUnread.js";
import { TASK_WATCH_INTENT_CACHE, reconcileTaskWatchIntent, selectTaskWatchIntentGuard } from "./taskWatchIntents.js";

export const TASK_WATCH_CACHE = "__task_watch_cache";
export const TASK_FAMILY_CACHE = "__task_family_cache";
const ids = ["storeGeneration", "scopeGeneration", "watchGeneration", "watchRetirementEpoch"];
const lineageFields = ["watched", "watch_started_after"];
const families = [
  { name: "total", fields: ["unread_count"], floor: (task, policy) => policy.newer(task.read_through, task.watch_started_after) },
  { name: "attention", fields: ["unread_mentions", "unread_agent_comments"], floor: (task) => task.read_through },
  { name: "activity", fields: ["session_started_ms", "last_activity_ms"] },
];
const dependentFields = ["watch_started_after", ...families.flatMap(({ fields }) => fields)];
const own = (value, field) => Object.hasOwn(value || {}, field);
const positive = (value) => Number.isSafeInteger(value) && value > 0;
const nonempty = (value) => typeof value === "string" && value.length > 0;
const presentCount = (value) => Number.isSafeInteger(value) && value >= 0;
const snapshot = (task, fields) => fields.map((field) => [field, own(task, field), task?.[field]]);
const sameSnapshot = (left, right) => left?.length === right?.length
  && left.every((entry, index) => entry.every((value, part) => Object.is(value, right[index]?.[part])));
const lineage = (task) => snapshot(task, lineageFields);
const sameLineage = (left, right) => sameSnapshot(lineage(left), lineage(right));
const sameScope = (left, right) => left?.deviceId === right?.deviceId && left?.projectId === right?.projectId;
const scopedTask = (scope, task) => nonempty(task?.id) && (!own(task, "project_id") || task.project_id === scope.projectId);
const sameTask = (task, id) => task?.id === id ? task : undefined;

function legacyCompare(left, right) {
  const a = taskUnreadKey(left) ?? "", b = taskUnreadKey(right) ?? "";
  return a < b ? -1 : a > b ? 1 : 0;
}

function markerPolicy(name) {
  if (name === "canonical") return { newer: newerTaskMarker, compare: compareTaskMarkers };
  if (name === "legacy") return { newer: (left, right) => latestTaskMark(left, right) || right, compare: legacyCompare };
  throw new Error("Unknown existing task marker policy");
}

const controlPresent = (context) => ids.every((key) => nonempty(context?.[key])) && presentCount(context?.watchPendingCount);
const sameLifetime = (proof, context) => ids.every((key) => nonempty(proof?.[key]) && proof[key] === context?.[key]);
const uncertain = (proof, context) => proof.uncertain !== false || context.watchUncertain !== false;
const pending = (context) => context.watchPendingCount !== 0 || context.localWatchPending === true;

function currentSource(proof, task, context, scope) {
  return controlPresent(context) && positive(proof?.observed_read_order) && sameLifetime(proof, context)
    && proof.taskId === task?.id && sameScope(proof.scope, scope)
    && ["read", "ack"].includes(proof.source);
}

function watchProof(task, context, scope) {
  const proof = task?.[TASK_WATCH_CACHE];
  return currentSource(proof, task, context, scope) && sameSnapshot(proof.lineage, lineage(task)) ? proof : null;
}

const taskProofScope = (task) => task?.[TASK_WATCH_CACHE]?.scope;
const intentUncertain = (scope, task, context) => reconcileTaskWatchIntent(scope, task,
  context?.watchIntentOf?.(task.id), context).uncertain;
const certainWatchProof = (proof, context) => Boolean(proof && !uncertain(proof, context)
  && (!pending(context) || proof.source === "ack"));

// This classifies an unchanged cached source only. Actual freshness and
// certification still require the selector's lifetime and shared fence checks.
export const hasTaskWatchSnapshot = (task) => sameSnapshot(task?.[TASK_WATCH_CACHE]?.lineage, lineage(task));

export function isTaskWatchCertain(task, context) {
  const scope = taskProofScope(task);
  if (!scope || !scopedTask(scope, task)) return false;
  if (intentUncertain(scope, task, context)) return false;
  return certainWatchProof(watchProof(task, context, scope), context);
}

function ticketMatches(read, scope, task, context) {
  return scopedTask(scope, task) && controlPresent(context) && positive(read?.readOrder)
    && ["storeGeneration", "scopeGeneration", "watchGeneration"].every((key) =>
      nonempty(read.ticket?.[key]) && read.ticket[key] === context[key]);
}

function ownedAcknowledgment(acknowledgment, read, task, context) {
  return nonempty(acknowledgment?.ownerId) && acknowledgment.taskId === task.id
    && positive(acknowledgment.intentRead) && acknowledgment.readOrder === read.readOrder
    && sameLifetime(acknowledgment, context);
}

function observation(scope, task, { contentRead, context, acknowledgment }) {
  if (typeof task?.watched !== "boolean" || !ticketMatches(contentRead, scope, task, context)) return null;
  const acknowledged = ownedAcknowledgment(acknowledgment, contentRead, task, context);
  const ticket = contentRead.ticket;
  if (!acknowledged && (ticket.watchRetirementEpoch !== context.watchRetirementEpoch
    || ticket.watchPending !== false || pending(context))) return null;
  return observedSource(scope, task, contentRead, context, acknowledged);
}

function observedSource(scope, task, contentRead, context, acknowledged) {
  const ticket = contentRead.ticket;
  const normalized = task.watched === false ? clearWatchDependents(task) : task;
  const proof = { ...sourceIdentity(scope, task, context, contentRead.readOrder),
    source: acknowledged ? "ack" : "read", uncertain: ticket.watchUncertain !== false || context.watchUncertain !== false,
    lineage: lineage(normalized) };
  return { task: normalized, proof, observed: true };
}

function sourceIdentity(scope, task, context, readOrder) {
  return { scope: { deviceId: scope.deviceId, projectId: scope.projectId }, taskId: task.id,
    ...Object.fromEntries(ids.map((key) => [key, context[key]])), observed_read_order: readOrder };
}

function clearWatchDependents(task) {
  const next = { ...task };
  for (const field of dependentFields) delete next[field];
  return next;
}

function newest(sources) {
  return sources.filter(Boolean).reduce((held, source) => !held
    || source.proof.observed_read_order > held.proof.observed_read_order ? source : held, null);
}

function retainedSource(task, context, scope) {
  const proof = watchProof(task, context, scope);
  if (!proof) return null;
  const downgrade = pending(context) && proof.source !== "ack";
  return { task, proof: downgrade ? { ...proof, uncertain: true } : proof, observed: false };
}

const stampedRows = (held, guard) => [held, guard].filter((task) => nonempty(task?.__task_watch_intent));

function watchSources(scope, held, incoming, options) {
  const stamped = stampedRows(held, options.watchGuard);
  if (stamped.length) {
    const observed = observation(scope, incoming, options);
    // An invalid descriptor can still describe an ordinarily admissible read.
    // Only an actual verified ack may release any existing owner's stamp.
    return observed?.proof.source === "ack" && stamped.some((task) =>
      task.__task_watch_intent === options.acknowledgment.ownerId) ? [observed] : [];
  }
  if (pending(options.context || {})) return [retainedSource(held, options.context, scope)
    || retainedSource(options.watchGuard, options.context, scope), observation(scope, incoming, options)];
  return [retainedSource(held, options.context, scope), retainedSource(options.watchGuard, options.context, scope),
    retainedSource(incoming, options.context, scope), observation(scope, incoming, options)];
}

function copyLineage(target, source) {
  const task = { ...target };
  for (const field of lineageFields) {
    delete task[field];
    if (own(source, field)) task[field] = source[field];
  }
  return task;
}

function selectedTask(held, incoming, selected, policy, fallback, omitLineage) {
  let task = copyLineage(incoming, omitLineage ? {} : selected?.task || fallback || incoming);
  task = selectedIntent(task, selected, fallback);
  for (const field of ["read_through", "dismissed_through"]) {
    task = selectedMarker(task, held, incoming, field, policy);
  }
  if (omitLineage || task.watched === false) task = clearWatchDependents(task);
  delete task[TASK_WATCH_CACHE]; delete task[TASK_FAMILY_CACHE];
  if (selected) task[TASK_WATCH_CACHE] = selected.proof;
  return task;
}

function selectedMarker(task, held, incoming, field, policy) {
  if (!own(held, field) && !own(incoming, field)) return task;
  const next = { ...task }, mark = policy.newer(held?.[field], incoming[field]);
  if (mark === undefined) delete next[field];
  else next[field] = mark;
  return next;
}

function selectedIntent(task, selected, fallback) {
  const next = { ...task };
  for (const field of ["__task_watch_intent", TASK_WATCH_INTENT_CACHE]) {
    delete next[field];
    if (!selected && own(fallback, field)) next[field] = fallback[field];
  }
  return next;
}

function knownFields(task, family) {
  const present = family.fields.filter((field) => own(task, field));
  if (family.name === "activity") return present.length > 0;
  return present.length === family.fields.length && present.every((field) => presentCount(task[field]));
}

function familyProof(source, family, policy) {
  return { ...source.proof, lineage: lineage(source.task), values: snapshot(source.task, family.fields),
    ...(family.floor ? { floor: family.floor(source.task, policy) } : {}) };
}

function retainedFamily(source, family, scope, context) {
  const metadata = source.task[TASK_FAMILY_CACHE];
  const paint = metadata?.paint?.[family.name];
  const proof = paint || metadata?.[family.name];
  return currentSource(proof, source.task, context, scope)
    && sameSnapshot(proof.lineage, lineage(source.task))
    && sameSnapshot(proof.values, snapshot(source.task, family.fields))
    ? familyValue(proof, source.task, Boolean(paint) || uncertain(proof, context) || uncertain(source.proof, context)) : null;
}

function sourceFamily(source, family, policy, scope, context) {
  if (!source) return null;
  if (source.observed) return knownFields(source.task, family)
    ? { ...familyValue(familyProof(source, family, policy), source.task, uncertain(source.proof, context)), observed: true } : null;
  return retainedFamily(source, family, scope, context);
}

function familyValue(proof, task, paint) {
  // Paint retains an honest source without becoming a known-value proof when
  // control uncertainty later clears or the row is copied through a guard.
  return { proof: paint ? { ...proof, uncertain: true } : proof, task, paint };
}

function unknownBarrier(task, family, scope, context, selected) {
  const proof = task?.[TASK_FAMILY_CACHE]?.unknown?.[family.name];
  if (!proof || retiredIncarnation(proof, selected)) return null;
  const valid = currentSource(proof, task, context, scope) && sameSnapshot(proof.lineage, lineage(task));
  return { proof, unknown: true, invalid: !valid };
}

const retiredIncarnation = (proof, selected) => selected && !sameSnapshot(proof.lineage, lineage(selected.task))
  && selected.proof.observed_read_order > proof.observed_read_order;

function compatibleFamily(source, family, task, policy) {
  return !source.unknown && sameSnapshot(source.proof.lineage, lineage(task))
    && (!family.floor || policy.compare(source.proof.floor, family.floor(task, policy)) === 0);
}

function familySelection({ sources, rawTasks, family, task, scope, context, policy, selectedWatch, fallback }) {
  const values = sources.map((source) => sourceFamily(source, family, policy, scope, context))
    .filter((value) => value && !retiredIncarnation(value.proof, selectedWatch));
  const barriers = rawTasks.map((raw) => unknownBarrier(raw, family, scope, context, selectedWatch)).filter(Boolean);
  const invalid = barriers.find((barrier) => barrier.invalid);
  const chosen = newest([...barriers.filter((barrier) => !barrier.invalid), ...values]);
  if (invalid && !recoversInvalidBarrier(chosen, barriers)) return invalid;
  if (!chosen) return fallbackFamily(fallback, family, task, policy);
  return compatibleFamily(chosen, family, task, policy) ? chosen : { proof: barrierProof(chosen.proof, task), unknown: true };
}

function fallbackFamily(fallback, family, task, policy) {
  if (!fallback || !knownFields(fallback, family) || !sameLineage(fallback, task)) return null;
  if (family.floor && policy.compare(family.floor(fallback, policy), family.floor(task, policy)) !== 0) return null;
  // One actual held/guard row may retain conservative display bytes. No
  // source identity or observation order is manufactured for this fallback.
  return { task: fallback, paint: true };
}

const recoversInvalidBarrier = (chosen, barriers) => chosen?.observed === true && barriers.every(({ proof }) =>
  positive(proof.observed_read_order) && chosen.proof.observed_read_order > proof.observed_read_order);

function barrierProof(proof, task) {
  // This fixed watermark is not a known-value proof. Its lineage binds the
  // unknown state in which an older source must not resurrect dropped values.
  const { values: _values, floor: _floor, ...identity } = proof;
  return { ...identity, lineage: lineage(task) };
}

function applyFamilies(task, sources, rawTasks, scope, context, policy, selectedWatch, fallback) {
  const stores = { metadata: {}, unknown: {}, paint: {} };
  for (const family of families) {
    for (const field of family.fields) delete task[field];
    const selected = familySelection({ sources, rawTasks, family, task, scope, context, policy, selectedWatch, fallback });
    applyFamily(task, family, selected, stores);
  }
  publishFamilyMetadata(task, stores);
}

function applyFamily(task, family, selected, { metadata, unknown, paint }) {
  if (!selected) return;
  if (selected.unknown) { unknown[family.name] = selected.proof; return; }
  if (selected.proof) (selected.paint ? paint : metadata)[family.name] = selected.proof;
  for (const field of family.fields) if (own(selected.task, field)) task[field] = selected.task[field];
}

function publishFamilyMetadata(task, { metadata, unknown, paint }) {
  if (Object.keys(unknown).length) metadata.unknown = unknown;
  if (Object.keys(paint).length) metadata.paint = paint;
  if (Object.keys(metadata).length) task[TASK_FAMILY_CACHE] = metadata;
}

const missingCounterValues = (task) => families.filter(({ floor }) => floor).some((family) => !knownFields(task, family));
const missingCounterProof = (task) => families.filter(({ floor }) => floor).some(({ name }) => !task[TASK_FAMILY_CACHE]?.[name]);
const selectedRead = (source) => source ? source.proof.observed_read_order : null;

function selectedResult(task, selected, context, incoming) {
  const watchCertain = isTaskWatchCertain(task, context);
  const incomplete = selected?.observed === true && missingCounterValues(incoming);
  const needsUnreadRefresh = task.watched !== false && (!watchCertain || incomplete || missingCounterProof(task));
  return { task, watchCertain, needsUnreadRefresh, acceptedWatch: selected?.observed === true,
    selectedWatchRead: selectedRead(selected) };
}

/** Incoming ordinary/PR fields are passed through unchanged. Owner validation
 * happens in the caller's transaction; this pure layer binds its exact ticket,
 * task and current post-settlement watch epoch, never bootstrapping uncertainty. */
export function reconcileTaskReadFacts(scope, held, incoming, options = {}) {
  const policy = markerPolicy(options.markerPolicy || "canonical");
  const prepared = intentInputs(scope, held, incoming, options);
  ({ held, incoming, options } = prepared);
  const guard = sameTask(options.watchGuard, incoming.id);
  const sources = watchSources(scope, held, incoming, { ...options, watchGuard: guard });
  const selected = newest(sources);
  const fallback = stampedRows(held, guard)[0] || held || (scopedTask(scope, guard) ? guard : undefined);
  const omitLineage = unavailablePendingLineage(options.contentRead, prepared.pending, selected, held, guard);
  const task = selectedTask(held, incoming, selected, policy, fallback, omitLineage);
  if (task.watched === true) applyFamilies(task, sources, [held, guard, incoming], scope, options.context, policy, selected, fallback);
  return selectedResult(task, selected, options.context, incoming);
}

const booleanWatchLineage = (task) => typeof task?.watched === "boolean";
// A current intent supplies protection, never the desired optimistic value.
// Without a real local lineage the refused wire family stays unavailable.
const unavailablePendingLineage = (contentRead, pendingIntent, selected, held, guard) => Boolean(contentRead && pendingIntent && !selected
  && !booleanWatchLineage(held) && !booleanWatchLineage(guard));

// Intent ordering is a local protection fence, never a watch observation. Each
// row is reconciled against the note read in this publication transaction.
function intentInputs(scope, held, incoming, options) {
  const context = options.context, note = context?.watchIntentOf?.(incoming.id);
  const actual = [held, incoming, options.watchGuard, ...(options.watchGuards || [])].filter((task) => task?.id === incoming.id);
  const protection = selectTaskWatchIntentGuard(scope, incoming.id, actual, note, context);
  const reconcile = (task) => task && reconcileTaskWatchIntent(scope, task, note, context).task;
  const guarded = protection.guard || options.watchGuard;
  const pendingOther = protection.pending && protection.pending.ownerId !== options.acknowledgment?.ownerId;
  return { pending: protection.pending, held: reconcile(sameTask(held, incoming.id)), incoming: reconcile(incoming), options: { ...options,
    context: pendingOther ? { ...context, localWatchPending: true } : context, watchGuard: reconcile(guarded) } };
}
