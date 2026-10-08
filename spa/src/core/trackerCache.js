// Where the tracker's records live in the local cache.
//
// A task belongs to a project, and a project is named by the machine it is on
// — both machines mint a `proj-1` — so every record here is addressed by
// (device, project). That is also what makes eviction one act: when a project
// stops being listed on a device, the cache's single range delete over
// (device, project) takes its task list and every task page with it.
//
// The records:
//
//   `tracker-tasks`            the project's whole list, plus its columns
//   `tracker-tasks-query`      one narrowed list, by its params
//   `tracker-tasks-page`       one page of a paged list, by its params (#85)
//   `tracker-task` / <id>      one task and its timeline, as `tasks.get`
//                               answered them
//
// The list is written by the sync layer on every pass (core/trackerSync.js) and
// is what the Tasks tab paints before the bridge has answered. The per-task
// record is written through by the task page itself on first open, which is
// how every other detail surface warms its own cache.

import { mergeCachedAtomically, readCached } from "./localCache.js";
import { taskUnreadKey, latestTaskMark } from "./trackerUnread.js";

export const TRACKER_TASKS_KIND = "tracker-tasks";
export const TRACKER_TASKS_QUERY_KIND = "tracker-tasks-query";
export const TRACKER_TASK_KIND = "tracker-task";
export const TRACKER_TASKS_PAGE_KIND = "tracker-tasks-page";

/** The project's list. */
export const tasksAddress = (deviceId, projectId) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_TASKS_KIND,
});

/** One narrowed list answer. The whole-list record above is the project's
 *  durable catalog; a filter response must not replace it or the menus would
 *  forget every value the current filter hid. Query params are assembled in
 *  a stable order by trackerFilters, so their JSON spelling is a stable cache
 *  key too. */
export const tasksQueryAddress = (deviceId, projectId, params) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_TASKS_QUERY_KIND,
  sub: JSON.stringify(params || {}),
});

/** One page of a paged `tasks.list` answer (#85), under the params that asked
 *  for it — its filter, cursor and limit — so every combination is a record of
 *  its own. A pull folds the list from what it reads back from here
 *  (core/trackerPages.js). */
export const tasksPageAddress = (deviceId, projectId, params) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_TASKS_PAGE_KIND,
  sub: JSON.stringify(params || {}),
});

/** One task and its timeline, under the project it belongs to. */
export const taskAddress = (deviceId, projectId, taskId) => ({
  deviceId,
  entityId: String(projectId || ""),
  kind: TRACKER_TASK_KIND,
  sub: String(taskId || ""),
});

/** The list record. `columns` rides beside the tasks because the two are read
 *  together on every paint — a board cannot be drawn from one without the
 *  other — and because the columns change far more rarely than the tasks, so
 *  holding them costs a field and saves a round trip on every cold open.
 *
 *  `readOrder` is the number the read that answered it took before it asked
 *  (core/taskReadOrder.js): when the bridge was asked, not when the answer
 *  reached the browser (#129). A list written here rather than read — a card
 *  moved, a task filed — takes its number from the same count as it is
 *  written, so every list is ordered against every other on one scale. */
export const tasksRecord = (tasks, columns, readOrder) => ({
  tasks: tasks || [],
  columns: columns || [],
  ...(Number.isFinite(readOrder) ? { read_order: readOrder } : {}),
});

/** When a cached list (`{ at, value }`) was asked for or written: its number.
 *  Only a list written before #129 has none, and stands in with the cache's
 *  own stamp on it — the clock the count is floored at, the nearest the two
 *  scales come to meeting. */
export const listAskedAt = (cached) => Number(cached?.value?.read_order) || cached?.at || 0;

export const taskRecord = (task, timeline) => ({ task: task || null, timeline: timeline || [] });

const rowIdentity = (row) => row?.id ?? row?.task_id ?? row?.workspace_id;
const TASK_REVIEW_CACHE = "__review_summary_cache";
const hasSummaryVersion = (summary) => Boolean(summary?.task_id) && Number.isFinite(summary.version);
const rowWrittenLater = (held, incoming) => Date.parse(incoming.updated_at) > Date.parse(held.updated_at);
const olderSummary = (held, incoming) => held.task_id === incoming.task_id
  && Number.isFinite(incoming.version) && incoming.version < held.version;
const keepRowSummary = (held, incoming, field, allowMissing) => incoming[field]
  ? olderSummary(held[field], incoming[field]) : !allowMissing && !rowWrittenLater(held, incoming);
const sameRow = (held, incoming) => Boolean(held) && rowIdentity(held) === rowIdentity(incoming);
const advancesSummaryFloor = (floor, summary) => hasSummaryVersion(floor) && hasSummaryVersion(summary)
  && summary.task_id === floor.task_id && summary.version > floor.version;

/** Server PR versions are independent of when a client asked for a row.
 * Only an existing visible summary of the same PR can establish that order. */
export const advancesReviewSummary = (held, incoming, field = "review_summary") => Boolean(incoming)
  && sameRow(held, incoming) && advancesSummaryFloor(held[field], incoming[field]);

function withHeldSummary(held, incoming, field) {
  const next = { ...incoming };
  if (Object.hasOwn(held, field)) next[field] = held[field];
  else delete next[field];
  return next;
}

/** A delayed task or workspace read can replace ordinary row fields without
 * rolling its PR summary back. Versions belong to one PR task; a new task
 * using the same workspace starts its own floor. Absence needs a newer read
 * or timestamp before it can erase a known summary. */
export function preserveReviewSummary(held, incoming, field = "review_summary", options = {}) {
  if (!incoming || !sameRow(held, incoming)) return incoming;
  if (options.keepHeld && !advancesReviewSummary(held, incoming, field)) return withHeldSummary(held, incoming, field);
  const summary = held?.[field];
  if (!hasSummaryVersion(summary)) return incoming;
  return keepRowSummary(held, incoming, field, options.allowMissing) ? { ...incoming, [field]: summary } : incoming;
}

/** Match only the rows an incoming list names: PR floors do not revive rows
 * that an authoritative list left out. */
export function preserveReviewSummaries(held, incoming, field = "review_summary", options) {
  const old = new Map((held || []).map((row) => [rowIdentity(row), row]));
  return (incoming || []).map((row) => preserveReviewSummary(old.get(rowIdentity(row)), row, field, options));
}

function cachedTaskReviewState(row) {
  const state = row?.[TASK_REVIEW_CACHE];
  return state?.task_id === rowIdentity(row) && hasSummaryVersion(state?.floor) ? state : null;
}

function taskReviewState(row) {
  const cached = cachedTaskReviewState(row);
  if (cached) return cached;
  if (!hasSummaryVersion(row?.review_summary)) return null;
  return { task_id: rowIdentity(row), floor: row.review_summary, updated_at: row.updated_at, cleared: false };
}

const reviewRead = (state) => Number(state?.read_order) || 0;
const reviewInstant = (state) => Date.parse(state?.updated_at) || 0;
const reviewReadOvertaken = (state, options) => options.keepHeld
  || Number.isFinite(options.readOrder) && options.readOrder < reviewRead(state);

/** Cached copies can have older ordinary fields than their accepted PR
 * authority. Compare that authority, never the ordinary row's timestamp. */
function strongerTaskReviewState(held, incoming) {
  if (!held) return incoming;
  if (advancesSummaryFloor(held.floor, incoming.floor)) return incoming;
  if (belowSummaryFloor(held.floor, incoming.floor)) return held;
  if (reviewInstant(held) !== reviewInstant(incoming)) return reviewInstant(incoming) > reviewInstant(held) ? incoming : held;
  if (reviewRead(held) !== reviewRead(incoming)) return reviewRead(incoming) > reviewRead(held) ? incoming : held;
  return incoming.cleared ? incoming : held;
}

function keepTaskReviewState(state, incoming, options) {
  const advances = advancesSummaryFloor(state.floor, incoming.review_summary);
  // A higher server version advances a visible PR across client read order.
  // A clear still fences requests that were made before it was accepted.
  if (reviewReadOvertaken(state, options) && (state.cleared || !advances)) return true;
  if (belowSummaryFloor(state.floor, incoming.review_summary)) return true;
  const later = rowWrittenLater(state, incoming);
  if (!incoming.review_summary) return !options.allowMissing && !later;
  return state.cleared && !advances && !later;
}

function withTaskReviewState(base, state) {
  const next = { ...base, [TASK_REVIEW_CACHE]: state };
  if (state.cleared) delete next.review_summary;
  else next.review_summary = state.floor;
  return next;
}

function acceptedTaskReviewState(state, incoming, options) {
  const keep = state && keepTaskReviewState(state, incoming, options);
  const floor = keep ? state.floor : incoming.review_summary || state?.floor;
  if (!hasSummaryVersion(floor)) return null;
  return {
    task_id: rowIdentity(incoming), floor,
    cleared: keep ? state.cleared : !incoming.review_summary,
    updated_at: latestTaskTimestamp(state?.updated_at, incoming.updated_at),
    read_order: Math.max(reviewRead(state), Number(options.readOrder) || 0),
  };
}

/** PR floors survive clears and stale ordinary fields on every task row.
 * Wire rows are filtered once against the held authority. Already cached
 * copies carry their own authority, so merging two cache replicas cannot
 * revive a clear. `baseRow` keeps a page's independently chosen ordinary
 * fields while applying only the PR field and its private authority. */
export function preserveTaskReviewSummary(held, incoming, options = {}) {
  if (!incoming) return incoming;
  const state = sameRow(held, incoming) ? taskReviewState(held) : null;
  const incomingState = cachedTaskReviewState(incoming);
  const next = incomingState ? strongerTaskReviewState(state, incomingState)
    : acceptedTaskReviewState(state, incoming, options);
  const base = options.baseRow || incoming;
  return next ? withTaskReviewState(base, next) : base;
}

const newerListRead = (held, incoming) => Number.isFinite(held?.read_order)
  && Number.isFinite(incoming?.read_order) && incoming.read_order > held.read_order;
const olderListRead = (held, incoming) => Number.isFinite(held?.read_order)
  && Number.isFinite(incoming?.read_order) && incoming.read_order < held.read_order;
const latestListRead = (held, incoming) => Number.isFinite(held?.read_order)
  ? Math.max(held.read_order, incoming.read_order || 0) : incoming.read_order;

function preserveListSummaries(held, incoming) {
  // An older absence must not lower the read floor and make the next older
  // absence appear authoritative enough to erase the retained summary.
  const readOrder = latestListRead(held, incoming);
  return {
    ...incoming,
    ...(Number.isFinite(readOrder) ? { read_order: readOrder } : {}),
    tasks: preserveListTaskRows(held, incoming),
  };
}

function preserveListTaskRows(held, incoming) {
  const old = new Map((held?.tasks || []).map((row) => [rowIdentity(row), row]));
  const options = {
    readOrder: incoming.read_order,
    allowMissing: newerListRead(held, incoming),
    keepHeld: olderListRead(held, incoming),
  };
  return (incoming.tasks || []).map((row) => preserveTaskReviewSummary(old.get(rowIdentity(row)), row, options));
}

function withReadThroughFloor(held, incoming) {
  const floor = latestTaskMark(held?.task?.read_through, incoming?.task?.read_through);
  const task = incoming?.task;
  if (!task || !floor || floor === task.read_through) return incoming;
  return { ...incoming, task: { ...task, read_through: floor } };
}

const belowSummaryFloor = (floor, summary) => hasSummaryVersion(floor) && hasSummaryVersion(summary) && olderSummary(floor, summary);

function detailReviewState(record) {
  const task = record?.task || {};
  const state = record?.[TASK_REVIEW_CACHE];
  if (state && state.task_id === task.id) return state;
  return { task_id: task.id, floor: task.review_summary ?? null, updated_at: task.updated_at };
}

function detailReviewOptions(state, task) {
  const summary = task?.review_summary;
  const later = Date.parse(task?.updated_at) > Date.parse(state.updated_at);
  return {
    allowMissing: later,
    keepHeld: belowSummaryFloor(state.floor, summary)
      || Boolean(state.floor) && !later && !advancesSummaryFloor(state.floor, summary),
  };
}

function latestTaskTimestamp(held, incoming) {
  const before = Date.parse(held);
  const next = Date.parse(incoming);
  return Number.isFinite(before) && (!Number.isFinite(next) || before > next) ? held : incoming;
}

/** Detail replies can carry older ordinary task fields. Keep PR authority
 * beside the record, so such fields cannot lower the timestamp of a clear,
 * and a higher numeric PR version can still advance an older task copy. */
function withDetailReviewState(state, record) {
  if (!record?.task) return record;
  const task = record.task;
  const floor = hasSummaryVersion(task.review_summary) ? task.review_summary : state.floor;
  if (!hasSummaryVersion(floor)) return record;
  return { ...record, [TASK_REVIEW_CACHE]: {
    task_id: task.id, floor, updated_at: latestTaskTimestamp(state.updated_at, task.updated_at),
  } };
}

function preserveDetailReview(held, incoming) {
  const state = sameRow(held?.task, incoming?.task) ? detailReviewState(held) : detailReviewState(incoming);
  const task = preserveReviewSummary(held?.task, incoming?.task, "review_summary", detailReviewOptions(state, incoming?.task));
  const next = task === incoming?.task ? incoming : { ...incoming, task };
  return withDetailReviewState(state, next);
}

function preserveTaskFloors(held, incoming) {
  return withReadThroughFloor(held, preserveDetailReview(held, incoming));
}

/** What the cache holds for a project, or null when it has never been read
 *  there. Never throws: a cache miss and a broken cache are the same answer. */
export async function readTasksRecord(deviceId, projectId) {
  const record = await readCached(tasksAddress(deviceId, projectId));
  return record?.value || null;
}

export async function readTaskRecord(deviceId, projectId, taskId) {
  const record = await readCached(taskAddress(deviceId, projectId, taskId));
  return record?.value || null;
}

export async function readTasksQueryRecord(deviceId, projectId, params) {
  const record = await readCached(tasksQueryAddress(deviceId, projectId, params));
  return record?.value || null;
}

/** A list record and when the cache took it, `{ at, value }`, or null when it
 *  holds none: one read, so a paint never takes the list from one record and
 *  its stamp from another, or reads a stamp of 0 for a record that went
 *  between the two (#129). */
async function readListCached(address) {
  const record = await readCached(address);
  return record?.value ? { at: record.at || 0, value: record.value } : null;
}

export const readTasksCached = (deviceId, projectId) => readListCached(tasksAddress(deviceId, projectId));

export const readTasksQueryCached = (deviceId, projectId, params) =>
  readListCached(tasksQueryAddress(deviceId, projectId, params));

export const writeTasksRecord = (deviceId, projectId, record) =>
  mergeCachedAtomically(tasksAddress(deviceId, projectId), (held) => preserveListSummaries(held, record));

/** Pulls may carry an older server read mark than one this browser has already
 * accepted. Keep the accepted mark in the cached task while replacing its
 * other fields and timeline with the newest pull. The transaction makes two
 * tabs' writes obey the same floor. */
export const writeTaskRecord = (deviceId, projectId, taskId, record, { accept = () => true } = {}) =>
  mergeCachedAtomically(taskAddress(deviceId, projectId, taskId), (held) => {
    // A writer can be superseded while waiting for the cache transaction.
    if (!accept(held)) return null;
    return preserveTaskFloors(held, record);
  });

/** Only an accepted read report may raise the cached floor. Do not touch the
 * timeline, and never accept a synthetic key as a read mark. */
export const advanceTaskReadThrough = (deviceId, projectId, taskId, mark) => {
  if (taskUnreadKey(mark) === null) return Promise.resolve(false);
  return mergeCachedAtomically(taskAddress(deviceId, projectId, taskId), (held) => {
    if (!held?.task) return null;
    const floor = latestTaskMark(held.task.read_through, mark);
    if (floor === held.task.read_through) return null;
    return { ...held, task: { ...held.task, read_through: floor } };
  });
};

export const writeTasksQueryRecord = (deviceId, projectId, params, record) =>
  mergeCachedAtomically(tasksQueryAddress(deviceId, projectId, params), (held) => preserveListSummaries(held, record));

/** When the cache last took an answer for a record, or 0 for one it has never
 *  held. A surface showing a cached copy says when that copy was read
 *  (core/transientRead.js), and this is when — the moment the answer landed,
 *  not the moment the surface got round to painting it. */
export const tasksRecordAt = async (deviceId, projectId) =>
  (await readCached(tasksAddress(deviceId, projectId)))?.at || 0;

export const taskRecordAt = async (deviceId, projectId, taskId) =>
  (await readCached(taskAddress(deviceId, projectId, taskId)))?.at || 0;
