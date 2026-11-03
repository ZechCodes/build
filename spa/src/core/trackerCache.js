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

import { mergeCachedAtomically, mergeCachedRecordsTogether, readCached, readCachedMany, recordWriteOf } from "./localCache.js";
import { taskUnreadKey, latestTaskMark } from "./trackerUnread.js";
import { readsAddress } from "./taskReadOrder.js";
import { mergeTaskContentRecordsTogether, observeTaskContent, reconcileTaskContentWrite,
  taskNeedsFullRefresh, TASK_CONTENT_CACHE, withTaskContentComponents, acceptsTaskContentRead,
  isObservedTaskContent, taskContentObservationOf, withTaskWatchIntentNote } from "./taskContentAuthority.js";
import { summaryTaskReadMutation, withTaskReadFacts, sameTaskReadFacts } from "./taskSummaryCache.js";
import { reconcileTaskReadFacts, hasTaskWatchSnapshot } from "./taskWatchFacts.js";
import { TASK_WATCH_INTENT_CACHE, reconcileTaskWatchIntent, selectTaskWatchIntentGuard } from "./taskWatchIntents.js";
import { blindTaskWatchInterference, retireTaskWatchFacts } from "./taskWatchAuthority.js";
const normalizedAddress = (deviceId, projectId, taskId) => ({ deviceId, entityId: projectId, kind: "tracker-summary", sub: taskId });

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

/** Legacy max clocks combine accepted and rejected answers. They cannot
 * prove that a visible PR was accepted after an independent clear. */
const visibleReviewObservation = (state) => state?.visible_observation || { updated_at: null, read_order: 0 };

/** Positive accepted read orders share the client clock. Otherwise the
 * accepted task timestamps stand in; protective maxima never participate. */
function reviewObservationOrder(before, next) {
  const reads = reviewRead(next) - reviewRead(before);
  const timestamps = reviewInstant(next) - reviewInstant(before);
  return reviewRead(before) > 0 && reviewRead(next) > 0 ? reads || timestamps : timestamps;
}

function newerTaskReviewState(held, incoming) {
  if (advancesSummaryFloor(held.floor, incoming.floor)) return incoming;
  if (belowSummaryFloor(held.floor, incoming.floor)) return held;
  const before = held.cleared ? taskReviewClearKey(held) : visibleReviewObservation(held);
  const next = incoming.cleared ? taskReviewClearKey(incoming) : visibleReviewObservation(incoming);
  const order = reviewObservationOrder(before, next);
  if (order) return order > 0 ? incoming : held;
  return incoming.cleared ? incoming : held;
}

/** An accepted absence has a stable identity even as rejected replies raise
 * its protective floors. Old cached clears gain that identity on first use. */
function taskReviewClearKey(state) {
  return state.clear_key || {
    task_id: state.task_id, review_task_id: state.floor.task_id, version: state.floor.version,
    updated_at: state.updated_at ?? null, read_order: reviewRead(state),
  };
}

function newerTaskReviewClearKey(held, incoming) {
  const before = taskReviewClearKey(held);
  const next = taskReviewClearKey(incoming);
  const difference = reviewObservationOrder(before, next) || next.version - before.version;
  return difference > 0 ? next : before;
}

function combinedTaskReviewClear(held, incoming) {
  return {
    ...held,
    floor: advancesSummaryFloor(held.floor, incoming.floor) ? incoming.floor : held.floor,
    updated_at: latestTaskTimestamp(held.updated_at, incoming.updated_at),
    read_order: Math.max(reviewRead(held), reviewRead(incoming)),
    clear_key: newerTaskReviewClearKey(held, incoming),
  };
}

function clearFencesCachedReview(clear, visible) {
  // A raw summary already accepted after this exact clear must survive an
  // interrupted repair. Mutable protective floors do not change that event.
  const absence = taskReviewClearKey(clear);
  if (JSON.stringify(visible.observed_clear) === JSON.stringify(absence)) return false;
  const observation = visibleReviewObservation(visible);
  return reviewObservationOrder(absence, observation) <= 0;
}

/** An old cached visible copy cannot use its version to cross a newer clear.
 * Positive observation orders compare that pair; legacy copies fall back to
 * authority timestamps. Only then do visible versions compete. Two clears
 * retain both the highest floor and their newest observation barriers. */
function strongerTaskReviewState(held, incoming) {
  if (!held) return incoming;
  if (held.cleared && incoming.cleared) return combinedTaskReviewClear(held, incoming);
  if (Boolean(held.cleared) !== Boolean(incoming.cleared)) {
    const clear = held.cleared ? held : incoming;
    const visible = held.cleared ? incoming : held;
    if (clearFencesCachedReview(clear, visible)) return clear;
  }
  return newerTaskReviewState(held, incoming);
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

function withTaskReviewLineage(held, next, keep, observation) {
  if (next.cleared) return { ...next, clear_key: taskReviewClearKey(keep ? held : { ...next, ...observation }) };
  const observedClear = held?.cleared ? taskReviewClearKey(held) : held?.observed_clear;
  return observedClear ? { ...next, observed_clear: observedClear } : next;
}

const sameReviewRevision = (held, incoming) => hasSummaryVersion(held) && hasSummaryVersion(incoming)
  && held.task_id === incoming.task_id && held.version === incoming.version;

function acceptedVisibleReviewObservation(held, next, candidate) {
  if (held?.cleared || next.cleared || !sameReviewRevision(held?.floor, next.floor)) return candidate;
  // Equal wire summaries can still bring older ordinary task fields. They
  // establish a new PR observation only when its own clock is newer.
  const before = visibleReviewObservation(held);
  return reviewObservationOrder(before, candidate) > 0 ? candidate : before;
}

function withVisibleReviewObservation(held, incoming, next, options, keep) {
  // Protective maxima still fence raw older replies. Only an accepted
  // visible summary establishes a new observation for cached comparisons.
  const observation = keep ? visibleReviewObservation(held)
    : acceptedVisibleReviewObservation(held, next,
      { updated_at: incoming.updated_at ?? null, read_order: Number(options.readOrder) || 0 });
  const linked = withTaskReviewLineage(held, next, keep, observation);
  if (linked.cleared) return linked;
  return { ...linked, visible_observation: observation };
}

function acceptedTaskReviewState(state, incoming, options) {
  const keep = state && keepTaskReviewState(state, incoming, options);
  const floor = keep ? state.floor : incoming.review_summary || state?.floor;
  if (!hasSummaryVersion(floor)) return null;
  const next = {
    task_id: rowIdentity(incoming), floor,
    cleared: keep ? state.cleared : !incoming.review_summary,
    updated_at: latestTaskTimestamp(state?.updated_at, incoming.updated_at),
    read_order: Math.max(reviewRead(state), Number(options.readOrder) || 0),
  };
  return withVisibleReviewObservation(state, incoming, next, options, keep);
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

export function preserveTaskFloors(held, incoming) {
  return withReadThroughFloor(held, preserveDetailReview(held, incoming));
}

/** What the cache holds for a project, or null when it has never been read
 *  there. Never throws: a cache miss and a broken cache are the same answer. */
export async function readTasksRecord(deviceId, projectId) {
  const record = await readCached(tasksAddress(deviceId, projectId));
  return record?.value || null;
}

function detailWithHeaderBarrier(value, header) {
  if (!value?.task || !taskNeedsFullRefresh(value.task, header)) return value || null;
  return { ...value, task: { ...value.task, [TASK_CONTENT_CACHE]: {
    ...value.task[TASK_CONTENT_CACHE], needs_full_refresh: true, barrier: header?.[TASK_CONTENT_CACHE],
  } } };
}

export async function readTaskRecord(deviceId, projectId, taskId) {
  const [record, summary] = await readCachedMany([taskAddress(deviceId, projectId, taskId), normalizedAddress(deviceId, projectId, taskId)]);
  return detailWithHeaderBarrier(record?.value, summary?.value?.task);
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

function contentList(held, record, address, context, written, contentRead) {
  if (contentRead) record = { ...record, read_order: contentRead.readOrder };
  const { positiveOnly, admitsMembership } = listMembershipAuthority(record, contentRead, address, context, written);
  if (!admitsMembership && !held && !positiveOnly) return null;
  const old = new Map((held?.tasks || []).map((task) => [task.id, task]));
  const incoming = (record.tasks || []).filter((task) => admitsMembership || positiveOnly || old.has(task.id));
  const blindWatch = (task) => isBlindTaskWatchWrite(old.get(task.id), task, address, context, contentRead);
  const results = incoming.map((task) => reconcileTaskContentWrite(old.get(task.id),
    contentRead ? observeTaskContent(task, { contentRead, coverage: "full" }) : task, { context, address, heldWritten: written }));
  const rows = results.flatMap((result, index) => {
    if (!result.task) return [];
    const raw = incoming[index];
    const facts = taskFactsForWrite(old.get(raw.id), raw, address, context, contentRead);
    const task = withTaskContentComponents(facts, result.task);
    return [listObservedReview(old.get(raw.id), task, contentRead ?? taskContentObservationOf(raw)?.contentRead, held)];
  });
  return { ...selectedListRecord(held, record, rows, results, admitsMembership, positiveOnly), retireWatch: incoming.some(blindWatch) };
}

function listObservedReview(held, task, contentRead, record) {
  if (!contentRead) return task;
  return preserveTaskReviewSummary(held, task, { readOrder: contentRead.readOrder,
    allowMissing: contentRead.readOrder > (record?.read_order || 0),
    keepHeld: contentRead.readOrder < (record?.read_order || 0) });
}

function listMembershipAuthority(record, contentRead, address, context, written) {
  const positiveOnly = contentRead === undefined && (record.tasks || []).some(isObservedTaskContent);
  const admitsMembership = contentRead === undefined ? !positiveOnly : acceptsTaskContentRead(contentRead, { context, address, heldWritten: written });
  return { positiveOnly, admitsMembership };
}

function taskFactsForWrite(held, raw, address, context, contentRead, watchGuard) {
  const scope = { deviceId: address.deviceId, projectId: address.entityId };
  const blind = isBlindTaskWatchWrite(held, raw, address, context, contentRead);
  const intent = reconcileTaskWatchIntent(scope, raw, context.watchIntentOf?.(raw.id), context);
  raw = intent.task;
  if (blind) return retireTaskWatchFacts(protectBlindTaskIntent(scope, held, raw, context));
  if (untrackedTaskFacts(held, raw, intent, contentRead)) return protectBlindTaskIntent(scope, held, raw, context);
  const annotation = taskContentObservationOf(raw);
  return reconcileTaskReadFacts(scope, held, taskFactsSource(raw, contentRead, annotation),
    { context, contentRead: contentRead ?? annotation?.contentRead, watchGuard, markerPolicy: "legacy" }).task;
}

const untrackedTaskFacts = (held, raw, intent, contentRead) => !contentRead && !isObservedTaskContent(raw)
  && !intent.retired && !sameTaskReadFacts(held, raw) && !hasTaskWatchMetadata(raw);
const taskFactsSource = (raw, contentRead, annotation) => contentRead || annotation?.coverage === "full"
  ? { watched: false, read_through: null, watch_started_after: null, dismissed_through: null, ...raw } : raw;

function protectBlindTaskIntent(scope, held, raw, context) {
  const protection = selectTaskWatchIntentGuard(scope, raw.id, [held, raw], context.watchIntentOf?.(raw.id), context);
  if (!protection.guard) return raw;
  const next = { ...raw };
  for (const field of ["__task_watch_intent", TASK_WATCH_INTENT_CACHE]) {
    if (Object.hasOwn(protection.guard, field)) next[field] = protection.guard[field];
  }
  return next;
}

function isBlindTaskWatchWrite(held, raw, address, context, contentRead) {
  if (contentRead || isObservedTaskContent(raw)) return false;
  const scope = { deviceId: address.deviceId, projectId: address.entityId };
  const intent = reconcileTaskWatchIntent(scope, raw, context.watchIntentOf?.(raw.id), context);
  return !intent.retired && blindTaskWatchInterference(scope, held, raw, context) && !hasTaskWatchSnapshot(raw);
}


const hasTaskWatchMetadata = (task) => ["__task_watch_cache", "__task_family_cache", "__task_watch_intent", TASK_WATCH_INTENT_CACHE]
  .some((field) => Object.hasOwn(task || {}, field));

function selectedListRecord(held, record, rows, results, admitsMembership, positiveOnly) {
  const byId = new Map(rows.map((task) => [task.id, task]));
  const prior = held?.tasks || [], priorIds = new Set(prior.map((task) => task.id));
  const positive = positiveOnly ? rows.filter((task) => !priorIds.has(task.id)) : [];
  const tasks = admitsMembership ? rows : [...prior.map((task) => byId.get(task.id) || task), ...positive];
  const removed = admitsMembership && prior.some((task) => !byId.has(task.id));
  return { record: preserveListSummaries(held, { ...(admitsMembership || !held ? record : held), tasks }),
    retire: removed || results.some((result) => result.retired) };
}

function writeContentLists(deviceId, projectId, entries, accept) {
  const watchTaskIds = [...new Set(entries.flatMap(({ record }) => (record.tasks || []).map((task) => task.id)))];
  return mergeTaskContentRecordsTogether({ deviceId, projectId }, entries.map(({ address }) => address), (held, context) => {
    if (!accept()) return null;
    const results = entries.map(({ address, record, contentRead }, index) => contentList(held[index]?.value, record,
      address, context, recordWriteOf(held[index]), contentRead));
    return { values: results.map((result) => result?.record || null), retire: results.some((result) => result?.retire),
      retireWatch: results.some((result) => result?.retireWatch) };
  }, { watchTaskIds });
}

function writeContentList(deviceId, projectId, address, record, { accept = () => true, contentRead } = {}) {
  return writeContentLists(deviceId, projectId, [{ address, record, contentRead }], accept);
}

/** The pane's existing query/catalogue replicas publish under one old scope
 * fence, so the first accepted removal cannot revoke the second one's ticket. */
export function writeTaskListRecords(deviceId, projectId, entries, { accept = () => true } = {}) {
  validateListPublications(deviceId, projectId, entries);
  return writeContentLists(deviceId, projectId, entries, accept);
}

const localListAddress = (address, deviceId, projectId) => address?.deviceId === deviceId
  && address.entityId === String(projectId) && [TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND].includes(address.kind);

function validateListPublications(deviceId, projectId, entries) {
  if (!Array.isArray(entries) || entries.length < 1 || entries.length > 2) throw new Error("Expected one or two task list replicas");
  const keys = new Set();
  for (const { address, contentRead } of entries) {
    if (!localListAddress(address, deviceId, projectId)) throw new Error("Foreign task list publication");
    const key = JSON.stringify([address.kind, address.sub || ""]);
    if (keys.has(key) || !contentRead) throw new Error("Expected distinct addressed task list captures");
    keys.add(key);
  }
}

export const writeTasksRecord = (deviceId, projectId, record, options) =>
  writeContentList(deviceId, projectId, tasksAddress(deviceId, projectId), record, options);

/** Capture the accepted action note before asking for a detail. Ordinary reads
 * leave the note in place, so another read cannot erase a mutation fence. */
export async function captureTaskDetailRead(deviceId, projectId, taskId) {
  const note = await readCached(readsAddress(taskAddress(deviceId, projectId, taskId)));
  return note?.value?.rows?.[taskId] || 0;
}

/** Pulls may carry an older server read mark than one this browser has already
 * accepted. Keep the accepted mark in the cached task while replacing its
 * other fields and timeline with the newest pull. The transaction makes two
 * tabs' writes obey the same floor. */
const detailAccepted = (held, note, taskId, options) => options.accept(held)
  && (options.detailBasis === undefined || options.detailBasis === (note?.value?.rows?.[taskId] || 0));

function publishedDetail(held, record, result, contentRead, facts) {
  const timeline = result.acceptedBody || !contentRead ? record.timeline : held?.timeline;
  const independent = facts || record.task;
  const next = preserveTaskFloors(held, { ...record, task: withTaskContentComponents(independent, result.task), timeline: timeline || [] });
  return next;
}

function contentDetail(detail, record, headerBarrier, context, address, contentRead, projection) {
  const held = detail?.value;
  const result = detailContentSelection(detail, record, headerBarrier, context, address, contentRead);
  if (!usableDetailSelection(result, held, contentRead)) return null;
  const retireWatch = blindDetailWatchChanged(held, record, contentRead, address, context);
  const facts = taskFactsForWrite(held?.task, record.task, address, context, contentRead, projection || headerBarrier);
  return { values: [publishedDetail(held, record, result, contentRead, facts), null, null], retire: result.retired, retireWatch,
    ordinaryDeferred: result.ordinaryDeferred };
}

function detailContentSelection(detail, record, headerBarrier, context, address, contentRead) {
  const task = contentRead ? observeTaskContent(record.task, { contentRead, coverage: "full" }) : record.task;
  return reconcileTaskContentWrite(detail?.value?.task, task, { context, address,
    heldWritten: recordWriteOf(detail), headerBarrier });
}

const usableDetailSelection = (result, held, contentRead) => Boolean(result.task)
  && !(contentRead && !held && typeof result.task.body !== "string");
const blindDetailWatchChanged = (held, record, contentRead, address, context) =>
  isBlindTaskWatchWrite(held?.task, record.task, address, context, contentRead);

const addressedFullRead = (read, address) => read?.revalidate === "full"
  && ["deviceId", "entityId", "kind", "sub"].every((key) => (read.address?.[key] ?? "") === (address[key] ?? ""));

function fullReadDetail(detail, shared, record, context, address, contentRead, mutation) {
  // An addressed baseline is an ordinary-content guard. Independent fields
  // retain their established authority when a local write changed that guard.
  const ordinaryRead = contentRead.written === recordWriteOf(detail) ? contentRead : { ...contentRead, ticket: null };
  let ordinaryDeferred = false;
  const normalized = mutation.reduce(shared, { context, headerBarrier: detail?.value?.task, ordinaryRead,
    onOrdinaryDeferred: () => { ordinaryDeferred = true; } });
  if (!normalized) return null;
  const next = contentDetail(detail, record, normalized[0].task, context, address, contentRead, normalized[0].task);
  const selected = next ? { ...next, values: [...next.values.slice(0, 2), ...normalized] } : { values: [null, null, ...normalized] };
  return { ...selected, ordinaryDeferred: ordinaryDeferred || next?.ordinaryDeferred === true };
}

/** onOrdinaryDeferred is a local scheduling hint after a committed write,
 * never authority. Its mounted caller still checks its captured owner. */
export const writeTaskRecord = async (deviceId, projectId, taskId, record,
  { accept = () => true, detailBasis, contentRead, onOrdinaryDeferred } = {}) => {
  const address = taskAddress(deviceId, projectId, taskId), scope = { deviceId, projectId };
  const mutation = addressedFullRead(contentRead, address) ? summaryTaskReadMutation(scope, record.task, contentRead) : null;
  const shared = mutation?.addresses || [normalizedAddress(deviceId, projectId, taskId)];
  let ordinaryDeferred = false;
  const written = await mergeTaskContentRecordsTogether(scope, [address, readsAddress(address), ...shared], ([detail, note, ...records], context) => {
    ordinaryDeferred = false;
    if (!detailAccepted(detail?.value, note, taskId, { accept, detailBasis })) return null;
    const current = withTaskWatchIntentNote(context, taskId, note?.value);
    const selected = mutation ? fullReadDetail(detail, records, record, current, address, contentRead, mutation)
      : contentDetail(detail, record, records[0]?.value?.task, current, address, contentRead);
    ordinaryDeferred = selected?.ordinaryDeferred === true;
    return selected;
  });
  if (written && ordinaryDeferred) {
    try { onOrdinaryDeferred?.(); }
    catch { /* A local refresh observer cannot change a committed write. */ }
  }
  return written;
};

/** Only an accepted read report may raise the cached floor. Do not touch the
 * timeline, and never accept a synthetic key as a read mark. */
const acceptsTaskReadFloor = (held, note, issuedRead, accept) => accept() && Boolean(held?.task)
  && !(note?.value?.action_read > issuedRead);

export const advanceTaskReadThrough = (deviceId, projectId, taskId, mark, { accept = () => true, issuedRead } = {}) => {
  if (taskUnreadKey(mark) === null) return Promise.resolve(false);
  const address = taskAddress(deviceId, projectId, taskId);
  return mergeTaskContentRecordsTogether({ deviceId, projectId }, [address, readsAddress(address)], ([detail, note], context) => {
    const held = detail?.value;
    if (!acceptsTaskReadFloor(held, note, issuedRead, accept)) return null;
    const floor = latestTaskMark(held.task.read_through, mark);
    if (floor === held.task.read_through) return null;
    const facts = reconcileTaskReadFacts({ deviceId, projectId }, held.task, { id: taskId, read_through: floor },
      { context: withTaskWatchIntentNote(context, taskId, note?.value), markerPolicy: "legacy" });
    return { values: [{ ...held, task: withTaskReadFacts(held.task, facts.task) },
      issuedRead === undefined ? null : { ...note?.value, action_read: issuedRead }] };
  });
};

export const writeTasksQueryRecord = (deviceId, projectId, params, record, options) =>
  writeContentList(deviceId, projectId, tasksQueryAddress(deviceId, projectId, params), record, options);

/** When the cache last took an answer for a record, or 0 for one it has never
 *  held. A surface showing a cached copy says when that copy was read
 *  (core/transientRead.js), and this is when — the moment the answer landed,
 *  not the moment the surface got round to painting it. */
export const tasksRecordAt = async (deviceId, projectId) =>
  (await readCached(tasksAddress(deviceId, projectId)))?.at || 0;

export const taskRecordAt = async (deviceId, projectId, taskId) =>
  (await readCached(taskAddress(deviceId, projectId, taskId)))?.at || 0;
