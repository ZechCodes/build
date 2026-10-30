// Shared watch coordination is independent of ordinary content and server
// bases. Missing/reset coordination is unknown, never proof of no live owner.
import { DELETE_CACHED_RECORD, mergeCachedRecordsTogether, readCached } from "./localCache.js";
import { randomToken } from "./idbRecords.js";
import { nextTaskReadTicket, readsAddress, withWritten, TASK_CONTENT_GENERATION_ADDRESS,
  taskContentScopeAddress, taskWatchControlAddress, initialTaskWatchControl } from "./taskReadOrder.js";
export { taskWatchControlAddress };
export const TASK_WATCH_CACHE = "__task_watch_cache";
export const TASK_FAMILY_CACHE = "__task_family_cache";
const catalogueAddress = ({ deviceId, projectId }) => ({ deviceId, entityId: projectId, kind: "tracker-tasks" });
const detailAddress = ({ deviceId, projectId }, taskId) => ({ deviceId, entityId: projectId, kind: "tracker-task", sub: taskId });
const ownerAddress = (owner) => ({ deviceId: owner.scope.deviceId, entityId: "", kind: "task-watch-owner",
  sub: JSON.stringify([owner.scope.projectId, owner.id]) });
const taskNote = (scope, taskId) => readsAddress(detailAddress(scope, taskId));
const controlIds = ["generation", "pendingGeneration", "watchRetirementEpoch"];
const validCount = (value) => Number.isSafeInteger(value) && value >= 0;
const validControl = (value) => Boolean(value && validCount(value.pendingCount)
  && controlIds.every((key) => typeof value[key] === "string" && value[key]));
const newControl = initialTaskWatchControl;

export async function readTaskWatchState(scope) {
  const control = (await readCached(taskWatchControlAddress(scope)))?.value;
  const valid = validControl(control);
  return { pendingCount: valid ? control.pendingCount : null,
    watchEpoch: valid ? control.watchRetirementEpoch : null,
    pendingGeneration: valid ? control.pendingGeneration : null,
    controlGeneration: valid ? control.generation : null,
    uncertain: !valid || control.uncertain !== false,
    eligible: valid && control.uncertain === false && control.pendingCount === 0 };
}

const validScope = (scope) => ["deviceId", "projectId"].every((key) => typeof scope?.[key] === "string" && scope[key].length > 0);
function validateBegin(scope, taskId, watched, metadataOnly) {
  if (!validScope(scope) || typeof taskId !== "string" || !taskId) throw new Error("Invalid watch mutation scope");
  if (typeof watched !== "boolean" || typeof metadataOnly !== "boolean") throw new Error("Expected explicit watch mutation intent");
}

const matchingDataLifetime = (owner, store, domain) => owner.storeGeneration === store?.value?.generation
  && owner.scopeGeneration === domain?.value?.generation;
const latestIntent = (note) => note?.value?.latest_watch_intent;
const ownedIntent = (owner, note) => latestIntent(note)?.ownerId === owner.id
  && latestIntent(note)?.intentRead === owner.intentRead;

export async function beginTaskWatchMutation(scope, taskId, { watched, metadataOnly = false, accept = () => true } = {}) {
  validateBegin(scope, taskId, watched, metadataOnly);
  const allocation = await nextTaskReadTicket(scope);
  if (!allocation || !accept()) return null;
  const owner = { id: randomToken(), scope: { deviceId: scope.deviceId, projectId: scope.projectId }, taskId, intentRead: allocation.readOrder, metadataOnly,
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
  && control.pendingCount < Number.MAX_SAFE_INTEGER && !existing && (latestIntent(note)?.intentRead || 0) <= owner.intentRead;

function registerWatchOwner([catalogue, note, task, heldControl, heldOwner], owner, watched) {
  const control = heldControl?.value || newControl();
  if (!canRegister(control, heldOwner, task, owner)) return null;
  const row = watchedRow(catalogue?.value, owner.taskId);
  Object.assign(owner, { controlGeneration: control.generation, pendingGeneration: control.pendingGeneration,
    beforeWatched: row ? row.watched === true : null });
  return registrationValues(catalogue?.value, note?.value, task?.value, owner, watched, control);
}

function registrationValues(catalogue, note, taskNote, owner, watched, control) {
  const stamp = { ownerId: owner.id, intentRead: owner.intentRead };
  const next = owner.metadataOnly ? null : paintWatch(catalogue, owner, watched);
  return [next, next ? withWritten(note, [owner.taskId], owner.intentRead) : null,
    { ...taskNote, latest_watch_intent: stamp },
    { ...control, pendingCount: control.pendingCount + 1, watchRetirementEpoch: randomToken() }, owner, null, null];
}

function paintWatch(held, owner, watched) {
  if (!held) return null;
  return { ...held, read_order: owner.intentRead, tasks: (held.tasks || []).map((task) => {
    if (task.id !== owner.taskId) return task;
    const next = { ...task, watched, __task_watch_intent: owner.id };
    delete next[TASK_WATCH_CACHE]; delete next[TASK_FAMILY_CACHE];
    for (const field of ["unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) delete next[field];
    return next;
  }) };
}

const ownerFields = ["id", "taskId", "intentRead", "metadataOnly", "beforeWatched", "storeGeneration", "scopeGeneration", "controlGeneration", "pendingGeneration"];
const matchingOwner = (owner, record) => Boolean(record?.value
  && ownerFields.every((key) => record.value[key] === owner[key])
  && ["deviceId", "projectId"].every((key) => record.value.scope?.[key] === owner.scope[key]));

const matchingControl = (owner, control) => validControl(control) && control.pendingCount > 0
  && control.generation === owner.controlGeneration && control.pendingGeneration === owner.pendingGeneration;

// The safe scaffold settles ownership/rollback only. Full acknowledged family
// publication is composed into this transaction by the action cache integration;
// an unintegrated answer never reports a successful canonical publication.
export async function settleTaskWatchMutation(owner, { answer, contentRead, rollback = false, accept = () => true } = {}) {
  if (!owner?.id || !owner.scope) return false;
  const addresses = [taskWatchControlAddress(owner.scope), ownerAddress(owner), catalogueAddress(owner.scope),
    taskNote(owner.scope, owner.taskId), TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress(owner.scope)];
  let restored = false;
  const written = await mergeCachedRecordsTogether(addresses, ([control, record, catalogue, note, store, domain]) => {
    if (!matchingOwner(owner, record) || !matchingControl(owner, control?.value)) return null;
    const trusted = record.value;
    const eligible = accept() && matchingDataLifetime(trusted, store, domain) && ownedIntent(trusted, note);
    const next = rollback && eligible ? rollbackWatch(catalogue?.value, trusted) : null;
    restored = rollback && eligible;
    return [{ ...control.value, pendingCount: control.value.pendingCount - 1, watchRetirementEpoch: randomToken() },
      DELETE_CACHED_RECORD, next, null, null, null];
  });
  void answer; void contentRead;
  return written && restored;
}

function rollbackWatch(held, owner) {
  if (owner.metadataOnly || owner.beforeWatched === null || !held) return null;
  const row = held.tasks?.find((task) => task.id === owner.taskId);
  return row?.__task_watch_intent === owner.id ? paintWatch(held, owner, owner.beforeWatched) : null;
}
