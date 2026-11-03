// Fixed local intent metadata, independent of ordinary headers and observed
// server families. Retirement never settles an owner or certifies raw paint.
export const TASK_WATCH_INTENT_CACHE = "__task_watch_intent_cache";
const stamp = "__task_watch_intent";
const fields = ["ownerId", "taskId", "intentRead", "storeGeneration", "scopeGeneration", "controlGeneration", "pendingGeneration"];
const lifetimes = [["storeGeneration", "storeGeneration"], ["scopeGeneration", "scopeGeneration"],
  ["controlGeneration", "watchGeneration"], ["pendingGeneration", "watchPendingGeneration"]];
const nonempty = (value) => typeof value === "string" && value.length > 0;
const validScope = (scope) => nonempty(scope?.deviceId) && nonempty(scope?.projectId);
const sameScope = (left, right) => left?.deviceId === right?.deviceId && left?.projectId === right?.projectId;
const own = (value, key) => Object.hasOwn(value || {}, key);

function validBinding(binding) {
  if (!validScope(binding?.scope)) return false;
  if (!Number.isSafeInteger(binding.intentRead) || binding.intentRead <= 0) return false;
  return fields.filter((field) => field !== "intentRead").every((field) => nonempty(binding[field]));
}

function copyBinding(binding) {
  return { ...Object.fromEntries(fields.map((field) => [field, binding[field]])),
    scope: { deviceId: binding.scope.deviceId, projectId: binding.scope.projectId } };
}

/** Bind only fields captured by actual registration; no watch epoch or receipt. */
export function watchIntentBinding(owner) {
  const binding = { ...owner, ownerId: owner?.id };
  return validBinding(binding) ? copyBinding(binding) : null;
}

const taskMatches = (task, binding) => task?.id === binding.taskId
  && (!own(task, "project_id") || task.project_id === binding.scope.projectId);

export function withTaskWatchIntent(task, binding) {
  if (!validBinding(binding) || !taskMatches(task, binding)) throw new TypeError("Invalid task watch intent binding");
  return { ...task, [stamp]: binding.ownerId, [TASK_WATCH_INTENT_CACHE]: copyBinding(binding) };
}

export function sameWatchIntent(left, right) {
  return validBinding(left) && validBinding(right) && sameScope(left.scope, right.scope)
    && fields.every((field) => left[field] === right[field]);
}

function currentBinding(binding, scope, taskId, context) {
  return validBinding(binding) && binding.taskId === taskId && sameScope(binding.scope, scope)
    && lifetimes.every(([field, key]) => binding[field] === context?.[key]);
}

function currentIntent(scope, taskId, latestNote, context) {
  const latest = latestNote?.latest_watch_intent;
  if (!currentBinding(latest, scope, taskId, context)) return null;
  if (!["pending", "acknowledged", "rejected"].includes(latest.disposition)) return null;
  return { binding: copyBinding(latest), disposition: latest.disposition };
}

const pendingIntent = (intent) => intent?.disposition === "pending" ? intent.binding : null;
const hasIntent = (task) => own(task, stamp) || own(task, TASK_WATCH_INTENT_CACHE);

function rowBinding(scope, task) {
  const binding = task?.[TASK_WATCH_INTENT_CACHE];
  if (!validBinding(binding) || !taskMatches(task, binding) || !sameScope(binding.scope, scope)) return null;
  return task[stamp] === binding.ownerId ? binding : null;
}

function exactSettlement(scope, task, exactOwner) {
  const binding = watchIntentBinding(exactOwner);
  if (!binding || !sameScope(binding.scope, scope) || !taskMatches(task, binding)) return null;
  if (task[stamp] !== binding.ownerId) return null;
  const cached = task[TASK_WATCH_INTENT_CACHE];
  if (own(task, TASK_WATCH_INTENT_CACHE) && !sameWatchIntent(cached, binding)) return null;
  return binding;
}

function withoutIntent(task) {
  const next = { ...task };
  delete next[stamp]; delete next[TASK_WATCH_INTENT_CACHE];
  return next;
}

function canRetire(binding, latest) {
  if (sameWatchIntent(binding, latest.binding)) return latest.disposition !== "pending";
  return latest.binding.intentRead > binding.intentRead;
}

/** latestNote is the per-task note VALUE, not its storage record. exactOwner is
 * supplied only by an already verified exact-owner settlement transaction.
 * `uncertain` classifies intent protection, never shared/server certainty. */
export function reconcileTaskWatchIntent(scope, task, latestNote, context, { exactOwner } = {}) {
  const latest = currentIntent(scope, task?.id, latestNote, context);
  const pending = pendingIntent(latest);
  if (!hasIntent(task)) return { task, retired: false, pending, uncertain: Boolean(pending) };
  const exact = exactSettlement(scope, task, exactOwner);
  if (exact) {
    const remaining = sameWatchIntent(exact, pending) ? null : pending;
    return { task: withoutIntent(task), retired: true, pending: remaining, uncertain: Boolean(remaining) };
  }
  return reconcileBoundIntent(scope, task, latest, pending, context);
}

function reconcileBoundIntent(scope, task, latest, pending, context) {
  const binding = rowBinding(scope, task);
  const current = currentBinding(binding, scope, task.id, context);
  if (!current || !latest || !canRetire(binding, latest)) return { task, retired: false, pending, uncertain: true };
  return { task: withoutIntent(task), retired: true, pending, uncertain: Boolean(pending) };
}

const scopedRow = (scope, taskId, task) => task?.id === taskId
  && (!own(task, "project_id") || task.project_id === scope.projectId);

/** Select one actual replica, never a synthetic maximum or an ordinary header.
 * A current pending note remains an independent barrier without a painted row. */
export function selectTaskWatchIntentGuard(scope, taskId, actualRows, latestNote, context) {
  const rows = (actualRows || []).filter((task) => scopedRow(scope, taskId, task));
  const latest = currentIntent(scope, taskId, latestNote, context);
  const pending = pendingIntent(latest);
  if (pending) {
    const guard = rows.find((task) => sameWatchIntent(rowBinding(scope, task), pending)) || null;
    return { guard, pending, uncertain: true };
  }
  const guard = rows.find((task) => reconcileTaskWatchIntent(scope, task, latestNote, context).uncertain) || null;
  return { guard, pending: null, uncertain: Boolean(guard) };
}
