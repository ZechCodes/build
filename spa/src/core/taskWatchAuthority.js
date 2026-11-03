// Shared watch coordination is independent of ordinary content and server
// bases. Missing/reset coordination is unknown, never proof of no live owner.
import { DELETE_CACHED_RECORD, mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { watchIntentBinding, withTaskWatchIntent, sameWatchIntent, reconcileTaskWatchIntent, selectTaskWatchIntentGuard } from "./taskWatchIntents.js";
import { withTaskWatchIntentNote } from "./taskContentAuthority.js";
import { randomToken } from "./idbRecords.js";
import { nextTaskReadTicket, readsAddress, withWritten, TASK_CONTENT_GENERATION_ADDRESS,
  taskContentScopeAddress, taskWatchControlAddress, initialTaskWatchControl, taskWatchControlUncertain } from "./taskReadOrder.js";
export { taskWatchControlAddress };
import { TASK_WATCH_CACHE, TASK_FAMILY_CACHE } from "./taskWatchFacts.js";
export { TASK_WATCH_CACHE, TASK_FAMILY_CACHE };
const catalogueAddress = ({ deviceId, projectId }) => ({ deviceId, entityId: projectId, kind: "tracker-tasks" });
const detailAddress = ({ deviceId, projectId }, taskId) => ({ deviceId, entityId: projectId, kind: "tracker-task", sub: taskId });
const ownerAddress = (owner) => ({ deviceId: owner.scope.deviceId, entityId: "", kind: "task-watch-owner",
  sub: JSON.stringify([owner.scope.projectId, owner.id]) });
const taskNote = (scope, taskId) => readsAddress(detailAddress(scope, taskId));
const controlIds = ["generation", "pendingGeneration", "watchRetirementEpoch"];
const validCount = (value) => Number.isSafeInteger(value) && value >= 0;
const validBlindEpoch = (value) => typeof value?.blindRetirementEpoch === "string" && value.blindRetirementEpoch.length > 0;
const validControl = (value) => Boolean(value && validCount(value.pendingCount)
  && controlIds.every((key) => typeof value[key] === "string" && value[key]));
const newControl = initialTaskWatchControl;

export function blindTaskWatchChanged(before, after) {
  return Boolean(before && before.id === after?.id && ((before.watched === true) !== (after.watched === true)
    || Object.hasOwn(before, "watch_started_after") !== Object.hasOwn(after, "watch_started_after")
    || !Object.is(before.watch_started_after, after.watch_started_after)));
}

// An absent or lineage-less replica has no prior boolean, but an exact same-task
// intent can still receive an older acknowledgment. Only that current binding
// makes explicit unowned watch paint a blind-interference event.
export function blindTaskWatchInterference(scope, before, after, context) {
  if (blindTaskWatchChanged(before, after)) return true;
  if (!explicitUnavailableWatch(before, after) || !positiveWatchPending(context)) return false;
  const protection = selectTaskWatchIntentGuard(scope, after.id, [], context.watchIntentOf?.(after.id), context);
  return Boolean(protection.pending);
}

function explicitUnavailableWatch(before, after) {
  if (typeof after?.watched !== "boolean") return false;
  return before == null || before.id === after.id && typeof before.watched !== "boolean";
}
const positiveWatchPending = (context) => Number.isSafeInteger(context?.watchPendingCount) && context.watchPendingCount > 0;

export function retireTaskWatchFacts(task) {
  const next = { ...task };
  delete next[TASK_WATCH_CACHE]; delete next[TASK_FAMILY_CACHE];
  return next;
}

export function retireTaskWatchOwnerStamp(task, owner) {
  if (!task || !owner) return task;
  return reconcileTaskWatchIntent(owner.scope, task, null, null, { exactOwner: owner }).task;
}

export async function readTaskWatchState(scope) {
  const control = (await readCached(taskWatchControlAddress(scope)))?.value;
  const valid = validControl(control);
  return { pendingCount: valid ? control.pendingCount : null,
    watchEpoch: valid ? control.watchRetirementEpoch : null,
    pendingGeneration: valid ? control.pendingGeneration : null,
    controlGeneration: valid ? control.generation : null,
    uncertain: !valid || taskWatchControlUncertain(control),
    eligible: valid && validBlindEpoch(control) && control.uncertain === false && control.pendingCount === 0 };
}

const validScope = (scope) => ["deviceId", "projectId"].every((key) => typeof scope?.[key] === "string" && scope[key].length > 0);
function validateBegin(scope, taskId, watched, metadataOnly) {
  if (!validScope(scope) || typeof taskId !== "string" || !taskId) throw new Error("Invalid watch mutation scope");
  if (typeof watched !== "boolean" || typeof metadataOnly !== "boolean") throw new Error("Expected explicit watch mutation intent");
}

const matchingDataLifetime = (owner, store, domain) => owner.storeGeneration === store?.value?.generation
  && owner.scopeGeneration === domain?.value?.generation;
const latestIntent = (note) => note?.value?.latest_watch_intent;
const ownedIntent = (owner, note) => sameWatchIntent(watchIntentBinding(owner), latestIntent(note));

export async function beginTaskWatchMutation(scope, taskId, { watched, metadataOnly = false, accept = () => true } = {}) {
  validateBegin(scope, taskId, watched, metadataOnly);
  const allocation = await nextTaskReadTicket(scope);
  if (!allocation || !accept()) return null;
  const owner = { id: randomToken(), scope: { deviceId: scope.deviceId, projectId: scope.projectId }, taskId, intentRead: allocation.readOrder, metadataOnly, watched,
    storeGeneration: allocation.storeGeneration, scopeGeneration: allocation.scopeGeneration };
  const catalogue = catalogueAddress(scope);
  const addresses = [catalogue, readsAddress(catalogue), taskNote(scope, taskId), taskWatchControlAddress(scope),
    ownerAddress(owner), TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress(scope)];
  let registered = false;
  const written = await mergeCachedRecordsTogether(addresses, (records) => {
    if (!accept() || !matchingDataLifetime(owner, records[5], records[6])) return null;
    const values = registerWatchOwner(records, owner, watched);
    registered = Boolean(values); return values;
  });
  return written && registered ? owner : null;
}

const watchedRow = (catalogue, id) => catalogue?.tasks?.find((task) => task.id === id);
const canRegister = (control, existing, note, owner) => validControl(control)
  && validBlindEpoch(control) && control.pendingCount < Number.MAX_SAFE_INTEGER && !existing
  && (latestIntent(note)?.intentRead || 0) <= owner.intentRead;

function registerWatchOwner([catalogue, note, task, heldControl, heldOwner], owner, watched) {
  const control = heldControl?.value || newControl();
  if (!canRegister(control, heldOwner, task, owner)) return null;
  const row = watchedRow(catalogue?.value, owner.taskId);
  Object.assign(owner, { controlGeneration: control.generation, pendingGeneration: control.pendingGeneration,
    beforeWatched: row ? row.watched === true : null, blindRetirementEpoch: control.blindRetirementEpoch });
  return registrationValues(catalogue?.value, note?.value, task?.value, owner, watched, control);
}

function registrationValues(catalogue, note, taskNote, owner, watched, control) {
  const stamp = { ...watchIntentBinding(owner), disposition: "pending" };
  const next = owner.metadataOnly ? null : paintWatch(catalogue, owner, watched);
  return [next, next ? withWritten(note, [owner.taskId], owner.intentRead) : null,
    { ...taskNote, latest_watch_intent: stamp },
    { ...control, pendingCount: control.pendingCount + 1, watchRetirementEpoch: randomToken() }, owner, null, null];
}

function paintWatch(held, owner, watched) {
  if (!held) return null;
  return { ...held, read_order: Math.max(held.read_order || 0, owner.intentRead), tasks: (held.tasks || []).map((task) => {
    if (task.id !== owner.taskId) return task;
    const next = withTaskWatchIntent({ ...task, watched }, watchIntentBinding(owner));
    delete next[TASK_WATCH_CACHE]; delete next[TASK_FAMILY_CACHE];
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) delete next[field];
    return next;
  }) };
}

const ownerFields = ["id", "taskId", "intentRead", "metadataOnly", "watched", "beforeWatched", "storeGeneration", "scopeGeneration", "controlGeneration", "pendingGeneration", "blindRetirementEpoch"];
const matchingOwner = (owner, record) => Boolean(record?.value
  && ownerFields.every((key) => record.value[key] === owner[key])
  && ["deviceId", "projectId"].every((key) => record.value.scope?.[key] === owner.scope[key]));

const matchingControl = (owner, control) => validControl(control) && control.pendingCount > 0
  && control.generation === owner.controlGeneration && control.pendingGeneration === owner.pendingGeneration;

/** Pure settlement fragment: exact owner cleanup and the post-settlement
 * fence commit beside the action's existing replicas. Cleanup authority is
 * distinct from permission to publish data from a retired session/scope. */
export function taskWatchSettlementFragment(owner) {
  return { addresses: [taskWatchControlAddress(owner.scope), ownerAddress(owner)],
    reduce: (records, options) => reduceWatchSettlement(owner, records, options) };
}

function reduceWatchSettlement(owner, [control, record], { context, note, contentRead, accept = () => true }) {
  if (!matchingOwner(owner, record) || !matchingControl(owner, control?.value)) return null;
  const trusted = record.value;
  const next = { ...control.value, pendingCount: control.value.pendingCount - 1, watchRetirementEpoch: randomToken() };
  const terminalNote = terminalWatchNote(note?.value, trusted, "rejected");
  const after = withTaskWatchIntentNote(settledWatchContext(context, next), trusted.taskId, terminalNote);
  const permissions = watchSettlementPermissions(trusted, control.value, { context, note, contentRead, accept, after });
  return { values: [next, DELETE_CACHED_RECORD], context: after, owner: trusted, note: terminalNote, ...permissions };
}

function watchSettlementPermissions(owner, control, { context, note, contentRead, accept, after }) {
  const eligible = accept() && currentOwnerData(owner, context) && ownedIntent(owner, note);
  const canPublish = eligible && validOwnerRead(owner, contentRead, after);
  const acknowledgment = canPublish && capturedOwnerWatchCurrent(owner, contentRead, control)
    ? watchAcknowledgment(owner, contentRead, after) : null;
  return { acknowledgment, canRestore: eligible && matchingBlindEpoch(owner, control), canPublish };
}

export function terminalWatchNote(note, owner, disposition) {
  return sameWatchIntent(note?.latest_watch_intent, watchIntentBinding(owner))
    ? { ...note, latest_watch_intent: { ...watchIntentBinding(owner), disposition } } : note;
}

const currentOwnerData = (owner, context) => owner.storeGeneration === context.storeGeneration
  && owner.scopeGeneration === context.scopeGeneration;
const matchingBlindEpoch = (owner, control) => validBlindEpoch(owner) && owner.blindRetirementEpoch === control.blindRetirementEpoch;
const settledWatchContext = (context, control) => ({ ...context, watchGeneration: control.generation,
  watchRetirementEpoch: control.watchRetirementEpoch, watchPendingCount: control.pendingCount,
  watchUncertain: taskWatchControlUncertain(control) });

// Admission epochs also rotate for other owners. Only a verified own reply
// can use the independently captured blind fence; ordinary reads cannot.
const capturedOwnerWatchCurrent = (owner, read, control) => matchingBlindEpoch(owner, control)
  && read.ticket.watchBlindRetirementEpoch === owner.blindRetirementEpoch
  && read.ticket.watchPendingGeneration === owner.pendingGeneration;

function validOwnerRead(owner, contentRead, context) {
  const ticket = contentRead?.ticket;
  return Number.isSafeInteger(contentRead?.readOrder) && contentRead.readOrder > owner.intentRead
    && ["storeGeneration", "scopeGeneration", "watchGeneration"].every((key) => ticket?.[key] && ticket[key] === context[key]);
}

function watchAcknowledgment(owner, contentRead, context) {
  return { ownerId: owner.id, taskId: owner.taskId, readOrder: contentRead.readOrder, intentRead: owner.intentRead,
    storeGeneration: context.storeGeneration, scopeGeneration: context.scopeGeneration, watchGeneration: context.watchGeneration,
    watchRetirementEpoch: context.watchRetirementEpoch };
}

const matchingWatchAnswer = (owner, answer) => answer?.task?.id === owner.taskId
  && answer.task.project_id === owner.scope.projectId && typeof answer.task.body === "string"
  && (answer.task.watched === true) === owner.watched;

// Matching full acknowledgments use the action cache's one shared transaction.
// Malformed/no-task answers only release their own pending owner; they cannot
// establish a watch observation or fabricate a full canonical response.
export async function settleTaskWatchMutation(owner, { answer, contentRead, rollback = false, accept = () => true } = {}) {
  if (!owner?.id || !owner.scope) return false;
  if (!rollback && matchingWatchAnswer(owner, answer)) {
    const { cacheTaskWaitingOnResponse } = await import("./taskWaitingOnCache.js");
    return cacheTaskWaitingOnResponse(owner.scope.deviceId, owner.scope.projectId, answer,
      { contentRead, issuedRead: contentRead?.readOrder, accept, watchOwner: owner });
  }
  const { cleanupTaskWatchMutation } = await import("./taskWaitingOnCache.js");
  return cleanupTaskWatchMutation(owner, { rollback, accept });
}
