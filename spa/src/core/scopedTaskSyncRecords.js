// The task projection's record reducers. The caller supplies the single cache
// transaction; neither this module nor its reducers perform asynchronous work.
import { DELETE_CACHED_RECORD, recordWriteOf } from "./localCache.js";
import { observeTaskContent, reconcileTaskContentWrite, retireTaskContent } from "./taskContentAuthority.js";
import { reconcileTaskReadFacts } from "./taskWatchFacts.js";
import { withTaskReadFacts } from "./taskSummaryCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";

const listKinds = new Set(["tracker-tasks", "tracker-tasks-query", "tracker-tasks-page", "tracker-summary-query"]);
export const revisionAddress = ({ deviceId, projectId }, id) => ({ deviceId, entityId: projectId, kind: "sync-task-revision", sub: id });
export const scopeFilters = (scope) => canonicalSummaryFilters({ ...scope.filters, order: scope.order || "number" });

function cachedFilters(address) {
  if (address.kind === "tracker-tasks") return {};
  const { project_id: _project, cursor: _cursor, limit: _limit, summary: _summary,
    read_order: _read, ...filters } = JSON.parse(address.sub || "{}");
  return canonicalSummaryFilters(filters);
}

export function matchingMembership(address, scope) {
  if (scope.kind !== "project_tasks" || !listKinds.has(address.kind)) return false;
  try { return JSON.stringify(cachedFilters(address)) === JSON.stringify(scopeFilters(scope)); }
  catch { return false; }
}

export const isMembership = (address) => listKinds.has(address.kind);
export const rowTask = (record) => record.kind === "task_head" ? record.row?.task : record.row;
const rowRemoved = (record) => record.operation === "delete";
const rowInvalidated = (record) => record.operation === "invalidate";

export function currentRevision(held, record, incarnation) {
  if (!held || held.incarnation !== incarnation) return true;
  if (held.deleted && !rowRemoved(record)) return BigInt(record.revision) > BigInt(held.revision);
  return BigInt(record.revision) >= BigInt(held.revision);
}

export function revisionValue(record, incarnation) {
  return { revision: record.revision, incarnation, deleted: rowRemoved(record), stale: rowInvalidated(record) };
}

function selectedTask(held, record, options) {
  const incoming = rowTask(record);
  if (!incoming || incoming.id !== record.key) throw new Error("Invalid sync task identity");
  const result = reconcileTaskContentWrite(held?.value?.task,
    observeTaskContent(incoming, { contentRead: options.capture.contentRead, coverage: "summary" }),
    { context: options.context, address: options.address, heldWritten: recordWriteOf(held) });
  const facts = reconcileTaskReadFacts(options.place, held?.value?.task, incoming,
    { context: options.context, contentRead: options.capture.contentRead, markerPolicy: "canonical" });
  return { task: withTaskReadFacts(result.task, facts.task), watch_uncertain: !facts.watchCertain,
    watch_read_order: facts.selectedWatchRead, needs_unread_refresh: facts.needsUnreadRefresh };
}

function invalidatedValue(held) {
  if (!held?.value) return null;
  const value = { ...held.value, stale: true, needs_unread_refresh: true };
  if (value.task) value.task = retireTaskContent(value.task);
  return value;
}

export function taskRecordValue(held, record, options) {
  if (rowRemoved(record)) return DELETE_CACHED_RECORD;
  if (rowInvalidated(record)) return invalidatedValue(held);
  const selected = selectedTask(held, record, options);
  const value = { ...held?.value, ...selected, read_order: options.capture.contentRead.readOrder, stale: false };
  if (options.address.kind === "tracker-task") {
    if (record.kind !== "task_head") return held ? invalidatedValue(held) : null;
    value.timeline = record.row.timeline;
    value.has_more = record.row.has_more;
  }
  return value;
}

function acceptedMember(record) {
  return record.member !== false && !rowRemoved(record) && !rowInvalidated(record);
}

function nextMembershipIds(held, records, snapshot) {
  const ids = new Set(snapshot ? [] : held || []);
  for (const record of records) {
    if (acceptedMember(record)) ids.add(record.key);
    else if (record.member === false || !rowInvalidated(record)) ids.delete(record.key);
  }
  return [...ids];
}

function queriedRows(held, records, options) {
  const old = new Map((options.snapshot ? [] : held || []).map((task) => [task.id, task]));
  for (const record of records) {
    if (acceptedMember(record)) old.set(record.key, options.taskOf(record.key));
    else if (record.member === false || !rowInvalidated(record)) old.delete(record.key);
  }
  return [...old.values()].filter(Boolean).sort((left, right) => right.number - left.number);
}

export function membershipValue(heldRecord, records, options) {
  const held = heldRecord?.value;
  const relevant = options.matches ? records : records.filter(rowRemoved);
  if (!writesMembership(held, relevant, options)) return null;
  const snapshot = options.matches && (options.snapshot || options.removed);
  const selected = options.removed ? [] : relevant;
  const value = { ...held, read_order: options.capture.contentRead.readOrder,
    stale: selected.some(rowInvalidated), refreshing: false };
  if (options.address.kind === "tracker-summary-query") summaryMembership(value, held, selected, { ...options, snapshot });
  else value.tasks = queriedRows(held?.tasks, selected, { ...options, snapshot });
  return value;
}

const writesMembership = (held, records, options) => Boolean(held || options.matches)
  && Boolean(records.length || options.snapshot || options.removed);

function summaryMembership(value, held, selected, options) {
  value.ids = nextMembershipIds(held?.ids, selected, options.snapshot)
    .sort((a, b) => (options.taskOf(b)?.number || 0) - (options.taskOf(a)?.number || 0));
  const watchFields = ["storeGeneration", "scopeGeneration", "watchGeneration", "watchRetirementEpoch"];
  Object.assign(value, { complete: true, next_cursor: null, published_generation: value.read_order,
    observed_read_order: value.read_order, identity_ids: held?.identity_ids || [],
    row_reads: Object.fromEntries(value.ids.map((id) => [id, value.read_order])),
    watch_observation: { ...Object.fromEntries(watchFields.map((field) => [field, options.context[field]])),
      uncertain: options.context.watchUncertain !== false || options.context.watchPendingCount !== 0 } });
  // A journal publication is a new membership authority, independent of a
  // retired tasks.list snapshot's continuation evidence and projection basis.
  delete value.snapshot_membership;
  delete value.basis;
}

export function removedDetail(held) {
  return { ...held?.value, task: null, timeline: [], stale: false };
}
