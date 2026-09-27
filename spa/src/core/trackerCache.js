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

import { mergeCachedAtomically, readCached, writeCached } from "./localCache.js";
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
  writeCached(tasksAddress(deviceId, projectId), record);

/** Pulls may carry an older server read mark than one this browser has already
 * accepted. Keep the accepted mark in the cached task while replacing its
 * other fields and timeline with the newest pull. The transaction makes two
 * tabs' writes obey the same floor. */
export const writeTaskRecord = (deviceId, projectId, taskId, record) =>
  mergeCachedAtomically(taskAddress(deviceId, projectId, taskId), (held) => {
    const floor = latestTaskMark(held?.task?.read_through, record?.task?.read_through);
    if (!record?.task || !floor || floor === record.task.read_through) return record;
    return { ...record, task: { ...record.task, read_through: floor } };
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
  writeCached(tasksQueryAddress(deviceId, projectId, params), record);

/** When the cache last took an answer for a record, or 0 for one it has never
 *  held. A surface showing a cached copy says when that copy was read
 *  (core/transientRead.js), and this is when — the moment the answer landed,
 *  not the moment the surface got round to painting it. */
export const tasksRecordAt = async (deviceId, projectId) =>
  (await readCached(tasksAddress(deviceId, projectId)))?.at || 0;

export const taskRecordAt = async (deviceId, projectId, taskId) =>
  (await readCached(taskAddress(deviceId, projectId, taskId)))?.at || 0;
