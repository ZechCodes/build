// Per-consumer display history, never counter authority or arithmetic input.
// Rendering is synchronous; guarded persistence is detached and may fail.
import { readCachedMany, mergeCachedRecordsTogether, recordWriteOf, subscribeCache } from "./localCache.js";
import { taskWatchIntentAddress, TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress,
  taskWatchControlAddress, TASK_READS_KIND } from "./taskReadOrder.js";
import { randomToken } from "./idbRecords.js";
import { taskUnreadCount, taskUnreadDisplayCount } from "./taskUnread.js";
import { timelineNeedsCounters } from "./taskTimelineWindow.js";
import { changedSince } from "./trackerModel.js";
import { TASK_FAMILY_CACHE, TASK_WATCH_CACHE, hasTaskWatchSnapshot } from "./taskWatchFacts.js";
import { messageOf } from "./text.js";

const DISPLAY = "task_unread_display";
const consumers = ["tasks-pane", "inbox"];
const count = (value) => Number.isSafeInteger(value) && value >= 0;
const positive = (value) => Number.isSafeInteger(value) && value > 0;
const own = (value, key) => Object.hasOwn(value || {}, key);
const nonempty = (value) => typeof value === "string" && value.length > 0;
const lineage = (task) => ["watched", "watch_started_after"].map((key) =>
  [key, own(task, key), typeof task?.[key], task?.[key]]);
const renderWatchSnapshot = (task) => [lineage(task), task?.__task_watch_intent, task?.[TASK_WATCH_CACHE]];
const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const input = (task, detail) => JSON.stringify([task, detail]);
const detailAddress = (scope, id) => ({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task", sub: id });
const lifetimeAddresses = (scope) => [TASK_CONTENT_GENERATION_ADDRESS, taskContentScopeAddress(scope), taskWatchControlAddress(scope)];
const contextOf = ([store, domain, control]) => {
  const watch = control?.value || {};
  return { storeGeneration: store?.value?.generation, scopeGeneration: domain?.value?.generation,
    watchGeneration: watch.generation, watchEpoch: watch.watchRetirementEpoch, pending: watch.pendingCount };
};
const lifetime = (context) => [context?.storeGeneration, context?.scopeGeneration];
const validLifetime = (context) => lifetime(context).every(nonempty);
const reportFailure = (error) => console.info("Task unread display persistence unavailable", error);
const familyAvailable = (task) => Boolean(task?.[TASK_FAMILY_CACHE]?.total || task?.[TASK_FAMILY_CACHE]?.paint?.total);
const familyUnknown = (task) => Boolean(task?.[TASK_FAMILY_CACHE]?.unknown?.total);
const timelineCurrent = (task, detail) => !timelineNeedsCounters(detail) && !changedSince(task, detail);

function unknownTotal(task, detail, previous) {
  if (familyUnknown(task)) return true;
  if (timelineCurrent(task, detail) && familyUnknown(detail.task)) return true;
  if (previous?.hadListed && !count(task?.unread_count)) return true;
  return Boolean(previous?.hadFamily && !familyAvailable(task));
}

function sourceOrigin(task, detail) {
  if (timelineCurrent(task, detail)) return "cached-timeline";
  if (count(task?.unread_count)) return "listed";
  return previousTimeline(task, detail) ? "cached-timeline" : null;
}

const previousTimeline = (task, detail) => Array.isArray(detail?.timeline) && detail.timeline.length > 0
  && same(lineage(task), lineage(detail.task));

function observedLineage(task, context, scope) {
  const proof = task?.[TASK_WATCH_CACHE];
  if (!hasTaskWatchSnapshot(task) || !validLifetime(context)) return null;
  if (!positive(proof.observed_read_order)) return null;
  if (!scopedObservation(proof, task, scope)) return null;
  if (!sameObservationLifetime(proof, context)) return null;
  if (context.pending !== 0 && proof.source !== "ack") return null;
  return { lineage: lineage(task), observedRead: proof.observed_read_order };
}

const sameObservationLifetime = (proof, context) => proof.storeGeneration === context.storeGeneration
  && proof.scopeGeneration === context.scopeGeneration && proof.watchGeneration === context.watchGeneration
  && nonempty(context.watchGeneration) && proof.watchRetirementEpoch === context.watchEpoch && nonempty(context.watchEpoch);

function scopedObservation(proof, task, scope) {
  if (task.__task_watch_intent || !["read", "ack"].includes(proof.source)) return false;
  return proof.taskId === task.id && proof.scope?.deviceId === scope.deviceId
    && proof.scope?.projectId === scope.projectId;
}

function endedAssociation(previous, task, context, scope) {
  if (!previous || previous.retired) return false;
  const observed = observedLineage(task, context, scope);
  if (!observed || same(previous.lineage, observed.lineage)) return false;
  return !previous.observedRead || observed.observedRead > previous.observedRead;
}

function validSlot(slot, scope, id, context) {
  if (!count(slot?.total) || !validLifetime(context)) return false;
  if (slot.deviceId !== scope.deviceId || slot.projectId !== scope.projectId || slot.taskId !== id) return false;
  return slot.storeGeneration === context.storeGeneration && slot.scopeGeneration === context.scopeGeneration;
}

function stateFromSlot(slot) {
  return { total: slot.total, lineage: slot.lineage, observedRead: slot.observedRead,
    origin: slot.origin, hadListed: slot.hadListed, hadFamily: slot.hadFamily, token: slot.token };
}

function selectedState(task, detail, selected, previous, context, scope) {
  const observed = observedLineage(task, context, scope);
  return { total: selected, lineage: observed?.lineage || lineage(task), observedRead: observed?.observedRead,
    origin: sourceOrigin(task, detail), hadListed: count(task.unread_count), hadFamily: familyAvailable(task),
    token: previous?.token || randomToken() };
}

const scopedTask = (scope, task) => nonempty(task?.id)
  && (!own(task, "project_id") || task.project_id === scope.projectId);
const listedTask = (record, id) => record?.value?.tasks?.find((task) => task.id === id);
const sameSelection = (left, right) => same([left?.total, left?.lineage, left?.origin], [right?.total, right?.lineage, right?.origin]);
const noteSlot = (record, consumer) => record?.value?.[DISPLAY]?.[consumer];
const canRecord = (task, detail, total, previous) => task.watched === true
  && !unknownTotal(task, detail, previous) && Boolean(sourceOrigin(task, detail)) && count(total);
const shouldWrite = (slot, selected, context) => validLifetime(context)
  && !sameSelection(slot, selected);
function pruneTo(entries, tasks) {
  for (const id of entries.keys()) if (!tasks.has(id)) entries.delete(id);
}

/** The owner calls preview during selection and rendered only after its actual
 * synchronous painter succeeds. Merely loading/enumerating rows never records
 * a display. One fixed note slot per consumer; mounted maps hold admitted IDs. */
export function createTaskUnreadDisplayOwner({ scope, consumer, active = () => true, sessionOf = () => null, onChange = () => {} }) {
  if (!consumers.includes(consumer)) throw new TypeError("Unknown task unread display consumer");
  const states = new Map(), notes = new Map(), tasks = new Map(), pending = new Map();
  let context = {}, disposed = false, serial = 0, hydrating = false;

  const usable = () => !disposed && active();
  const previousOf = (task) => {
    const previous = states.get(task.id);
    if (endedAssociation(previous, task, context, scope)) return undefined;
    return previous?.retired ? undefined : previous;
  };
  const preview = (task, detail) => taskUnreadDisplayCount(task, detail,
    { unknown: unknownTotal(task, detail, previousOf(task)), previous: previousOf(task)?.total });
  const currentRender = (task) => Boolean(tasks.get(task.id))
    && same(renderWatchSnapshot(tasks.get(task.id)), renderWatchSnapshot(task));

  function rendered(task, detail, total, listedAddress) {
    if (!usable() || hydrating || !scopedTask(scope, task) || !currentRender(task)) return;
    const previous = previousOf(task);
    if (endedAssociation(states.get(task.id), task, context, scope)) states.set(task.id, { retired: true });
    if (!canRecord(task, detail, total, previous) || total !== taskUnreadCount(task, detail)) return;
    const next = selectedState(task, detail, total, previous, context, scope);
    const selected = same(previous, next) ? previous : next;
    states.set(task.id, selected);
    if (shouldWrite(noteSlot(notes.get(task.id), consumer), selected, context)) prepareWrite(task, detail, selected, listedAddress);
  }

  function prepareWrite(task, detail, selected, listedAddress) {
    if (!listedAddress) return;
    const prepared = { task, detail, selected, listedAddress, sourceKey: input(task, detail),
      noteWrite: recordWriteOf(notes.get(task.id)), session: sessionOf(), context: { ...context } };
    pending.set(task.id, prepared);
    void persist(prepared).catch(reportFailure).finally(() => {
      if (pending.get(task.id) === prepared) pending.delete(task.id);
    });
  }

  const current = (prepared) => usable() && sessionOf() === prepared.session
    && states.get(prepared.task.id) === prepared.selected && pending.get(prepared.task.id) === prepared;

  async function persist(prepared) {
    const id = prepared.task.id;
    const addresses = [prepared.listedAddress, detailAddress(scope, id), taskWatchIntentAddress(scope, id), ...lifetimeAddresses(scope)];
    const records = await readCachedMany(addresses);
    if (!preparedMatches(prepared, records)) return;
    const writes = records.map(recordWriteOf);
    const slot = persistedSlot(prepared, scope, writes);
    await mergeCachedRecordsTogether(addresses, (held) => {
      if (!current(prepared) || !held.every((record, index) => recordWriteOf(record) === writes[index])) return null;
      const note = held[2]?.value || {};
      const slots = Object.fromEntries(consumers.filter((key) => own(note[DISPLAY], key)).map((key) => [key, note[DISPLAY][key]]));
      return [null, null, { ...note, [DISPLAY]: { ...slots, [consumer]: slot } }, null, null, null];
    });
  }

  function preparedMatches(prepared, records) {
    if (!current(prepared) || recordWriteOf(records[2]) !== prepared.noteWrite) return false;
    if (!same(contextOf(records.slice(3)), prepared.context)) return false;
    const task = listedTask(records[0], prepared.task.id), detail = records[1]?.value || null;
    if (!same(input(task, detail), prepared.sourceKey)) return false;
    return taskUnreadCount(task, detail) === prepared.selected.total && !unknownTotal(task, detail, prepared.selected);
  }

  async function updateTasks(next) {
    const read = ++serial;
    hydrating = true;
    tasks.clear();
    for (const task of next || []) if (scopedTask(scope, task)) tasks.set(task.id, task);
    for (const entries of [states, notes, pending]) pruneTo(entries, tasks);
    const ids = [...tasks.keys()];
    try {
      const records = await readCachedMany([...lifetimeAddresses(scope), ...ids.map((id) => taskWatchIntentAddress(scope, id))]);
      acceptNotes(read, ids, records);
    } catch (error) {
      reportFailure(error);
      const failure = new Error(messageOf(error), { cause: error });
      failure.superseded = !usable() || read !== serial;
      throw failure;
    } finally {
      if (read === serial) {
        hydrating = false;
        if (usable()) onChange();
      }
    }
  }

  function acceptNotes(read, ids, records) {
    if (!usable() || read !== serial) return;
    const nextContext = contextOf(records);
    if (validLifetime(context) && !same(lifetime(context), lifetime(nextContext))) states.clear();
    context = nextContext;
    ids.forEach((id, index) => acceptNote(id, records[index + 3]));
  }

  function acceptNote(id, record) {
    notes.set(id, record);
    const task = tasks.get(id), previous = states.get(id);
    if (endedAssociation(previous, task, context, scope)) { states.set(id, { retired: true }); return; }
    if (previous) return;
    const slot = noteSlot(record, consumer);
    if (!validSlot(slot, scope, id, context)) return;
    const selected = stateFromSlot(slot);
    if (!endedAssociation(selected, task, context, scope)) states.set(id, selected);
  }

  let refreshing = null;
  const unsubscribe = subscribeCache({ deviceId: scope.deviceId, entityId: scope.projectId, kind: TASK_READS_KIND }, () => {
    if (!usable() || refreshing) return;
    // updateTasks reports failures and notifies on settlement; this detached
    // refresh only consumes its already-reported rejection.
    refreshing = updateTasks([...tasks.values()]).catch(() => {}).finally(() => { refreshing = null; });
  });

  return { preview, rendered, updateTasks, hydrating: () => hydrating,
    dispose() { disposed = true; serial += 1; hydrating = false; unsubscribe(); states.clear(); notes.clear(); tasks.clear(); pending.clear(); } };
}

function persistedSlot(prepared, scope, writes) {
  const state = prepared.selected;
  return { deviceId: scope.deviceId, projectId: scope.projectId, taskId: prepared.task.id,
    storeGeneration: prepared.context.storeGeneration, scopeGeneration: prepared.context.scopeGeneration,
    total: state.total, lineage: state.lineage, observedRead: state.observedRead, origin: state.origin,
    hadListed: state.hadListed, hadFamily: state.hadFamily, token: state.token,
    source: { kind: prepared.listedAddress.kind, sub: prepared.listedAddress.sub || "", listedWrite: writes[0], detailWrite: writes[1] } };
}
