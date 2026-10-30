// Normalized summaries and independent loaded query membership. This module
// is a cache writer/reader only: it neither asks the wire nor drains cursors.
// Detail/timeline records belong to trackerCache and are never replaced here.
import { mergeCachedRecordsTogether, readCached, readCachedMany } from "./localCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { compareTaskMarkers, isPresentTaskMarker, newerTaskMarker } from "./taskOpaqueMarkers.js";
import { preserveTaskReviewSummary } from "./trackerCache.js";
import { recordWriteOf } from "./localCache.js";
import { captureTaskContentRead, mergeTaskContentRecordsTogether, observeTaskContent, reconcileTaskContentWrite,
  taskContentFragment, withTaskContentComponents, sameTaskContentComponent } from "./taskContentAuthority.js";
import { summaryPatch, fullTaskSummary } from "./taskSummaryProjection.js";

export const TASK_SUMMARY_KIND = "tracker-summary";
export const TASK_SUMMARY_QUERY_KIND = "tracker-summary-query";
export const TASK_SUMMARY_IDENTITY_KIND = "tracker-summary-identity";

const address = ({ deviceId, projectId }, kind, sub) => ({ deviceId, entityId: String(projectId), kind, sub });
export const summaryAddress = (scope, id) => address(scope, TASK_SUMMARY_KIND, id);
const identityAddress = (scope, id) => address(scope, TASK_SUMMARY_IDENTITY_KIND, id);
const invalidationAddress = (scope) => address(scope, "tracker-summary-invalidation", "");
const observationAddress = (scope) => address(scope, "tracker-summary-observation", "");
export const summaryQueryAddress = (scope, filters) => address(scope, TASK_SUMMARY_QUERY_KIND,
  JSON.stringify(canonicalSummaryFilters(filters)));

/** Start a refresh without discarding the last successful cached paint.
 * A caller takes generation from nextTaskRead before dispatch. Older tickets
 * cannot reset a newer refresh, including across tabs. */
export async function beginSummaryQuery(scope, filters, generation, { accept = () => true, contentRead } = {}) {
  contentRead ??= await captureTaskContentRead(scope, { revalidate: "summary" });
  if (!Number.isSafeInteger(generation) || generation <= 0) throw new Error("Invalid summary query generation");
  const queryAddress = summaryQueryAddress(scope, filters);
  await mergeCachedRecordsTogether([queryAddress], ([held]) => {
    if (!accept() || held?.value?.generation >= generation) return null;
    return [{ ids: [], identity_ids: [], next_cursor: null, complete: false,
      ...held?.value, generation, refreshing: true }];
  });
  return { scope: { ...scope }, address: queryAddress, filters: canonicalSummaryFilters(filters), generation, cursor: null, contentRead };
}

function validatePage(ticket, page) {
  if (page?.summary !== true || !Array.isArray(page.tasks)) throw new Error("Expected a task summary page");
  if (page.tasks.some((task) => typeof task?.id !== "string" || !task.id)) throw new Error("Invalid task summary identity");
  validateBasis(page);
  for (const task of page.tasks) validateQueryFields(ticket.filters || {}, task);
  validateContinuation(ticket.cursor, page.next_cursor);
}

function validateContinuation(cursor, next) {
  if (next != null && (typeof next !== "string" || !next || next === cursor)) throw new Error("Invalid summary continuation");
}

function validateBasis(page) {
  if (Object.hasOwn(page, "basis") && (typeof page.basis !== "string" || !page.basis)) throw new Error("Invalid summary basis");
}

function validateQueryFields(filters, task) {
  if (filters.order === "anchor" && task.order_anchor_ms !== null && !Number.isSafeInteger(task.order_anchor_ms)) {
    throw new Error("Missing authoritative task order anchor");
  }
  for (const key of ["linked_workspace_id", "tracker_agent_id"]) {
    if (Object.hasOwn(filters, key) && task.matched_memberships?.[key] !== filters[key]) {
      throw new Error("Missing exact task membership provenance");
    }
  }
}

function acceptsPage(held, ticket, accept) {
  if (!accept() || held?.generation !== ticket.generation) return false;
  if (ticket.cursor === null) return held.refreshing === true;
  return !held.refreshing && !held.complete && held.next_cursor === ticket.cursor;
}

const distinct = (values) => [...new Set(values)];
const append = (held, incoming, first) => distinct([...(first ? [] : held || []), ...incoming]);

function membership(held, ticket, page, ids, identities, read) {
  const first = ticket.cursor === null;
  return { ...held,
    ids: append(held.ids, ids, first),
    identity_ids: append(held.identity_ids, identities, first),
    row_reads: { ...(first ? {} : held.row_reads), ...Object.fromEntries(ids.map((id) => [id, read])) },
    row_fields: { ...(first ? {} : held.row_fields), ...Object.fromEntries(page.tasks.map((task) => [task.id, queryFields(task)])) },
    basis: page.basis,
    next_cursor: page.next_cursor ?? null,
    complete: page.next_cursor == null,
    refreshing: false,
    published_generation: ticket.generation,
    observed_read_order: read,
  };
}

function queryFields(task) {
  return Object.fromEntries(["matched_memberships", "order_anchor_ms"]
    .filter((key) => Object.hasOwn(task, key)).map((key) => [key, task[key]]));
}

function entityFields(task) {
  const next = { ...task };
  delete next.matched_memberships;
  delete next.order_anchor_ms;
  return next;
}

function pageRow(held, task, read, basis, options) {
  const incoming = entityFields(task), older = olderRow(held, incoming, read);
  const value = summaryContent(held, summaryValue(held, incoming, read), incoming, options).row;
  if (!value.task) return value;
  const oldAuthority = older && value.watch_read_order !== read;
  const source = oldAuthority ? held?.task : incoming;
  const mixed = changedBasisAuthority(held, basis, source, value.task);
  // A current watch header does not certify the ordinary content retained
  // by a newer timestamp/read. Keep that merge uncertain until content wins.
  const uncertain = mixed || retainedOrdinaryBasis(held, older, value.watch_read_order === read)
    || unprovenPageContent(held, incoming, value, basis);
  return { ...value, basis: uncertain ? undefined : oldAuthority ? held?.basis : basis, basis_uncertain: uncertain };
}

const unprovenPageContent = (held, incoming, value, basis) => Boolean(held && !value.contentAccepted
  && (!sameTaskContentComponent(held.task, incoming, "header") || held.basis_uncertain || held.basis !== basis));

const retainedOrdinaryBasis = (held, older, currentWatch) => older && (currentWatch || held?.basis_uncertain === true);

const changedBasisAuthority = (held, basis, source, next) => Boolean(held && held.basis !== basis && authorityDiffers(source, next));
const authorityDiffers = (before, after) => ["read_through", "dismissed_through"]
  .some((key) => compareTaskMarkers(before?.[key], after?.[key]) !== 0)
  || watchAuthorityDiffers(before, after)
  || JSON.stringify(before?.review_summary ?? null) !== JSON.stringify(after?.review_summary ?? null);
const watchAuthorityDiffers = (before, after) => after.watched !== false
  && compareTaskMarkers(counterFloor(before), counterFloor(after)) !== 0;

const newerObservation = (observation, read, basis) => observation?.read_order > read ? null : { read_order: read, basis };
const unchangedBasis = (held, other) => Boolean(held.basis && held.basis === other?.basis);
const invalidatedTicket = (invalidation, ticket) => invalidation?.read_order > ticket.generation;
const changedContinuationBasis = (held, ticket, page) => ticket.cursor !== null && held.basis !== page.basis;

export function preserveTaskActionMarks(held, task, { watchAuthority = false } = {}) {
  if (watchAuthority && changedWatchLineage(held, task)) held = withoutWatchCounters(held || {});
  const next = withMonotonicMarks(held, task, watchAuthority);
  if (next.watched === false) return withoutWatchCounters(next);
  for (const { floor, fields } of counterFamilies) preserveCounterFamily(next, held, task, floor, fields);
  return next;
}

function withMonotonicMarks(held, task, watchAuthority) {
  const next = { ...task };
  for (const field of ["read_through", "watch_started_after", "dismissed_through"]) {
    if (field === "watch_started_after" && (next.watched === false || watchAuthority)) continue;
    const mark = newerTaskMarker(held?.[field], next[field]);
    if (isPresentTaskMarker(mark)) next[field] = mark;
  }
  return next;
}

const watchFields = ["watch_started_after", "unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"];
const changedWatchLineage = (before, after) => before?.watched !== after?.watched
  || (before?.watch_started_after ?? null) !== (after?.watch_started_after ?? null);

export function withTaskWatchLineage(task, source) {
  const next = { ...withoutWatchCounters(task), watched: source?.watched === true };
  if (next.watched) for (const field of watchFields) {
    if (Object.hasOwn(source, field)) next[field] = source[field];
  }
  return next;
}

function withoutWatchCounters(task) {
  const next = { ...task };
  for (const key of watchFields) delete next[key];
  return next;
}

const counterFloor = (task) => newerTaskMarker(task?.read_through, task?.watch_started_after);
const counterFamilies = [
  { floor: counterFloor, fields: ["unread_count"] },
  { floor: (task) => task?.read_through, fields: ["unread_mentions", "unread_agent_comments"] },
];

function preserveCounterFamily(next, held, task, floor, fields) {
  if (compareTaskMarkers(floor(held), floor(task)) <= 0) return;
  for (const field of fields) {
    if (Object.hasOwn(held, field)) next[field] = held[field];
    else delete next[field];
  }
}

const markerFields = ["read_through", "watch_started_after", "dismissed_through"];
const strongerMarks = (held, incoming) => markerFields.some((field) => compareTaskMarkers(incoming[field], held?.[field]) > 0);
const coversMarks = (held, incoming) => markerFields.every((field) =>
  compareTaskMarkers(incoming[field], held?.[field]) >= 0);

function markerAuthority(held, incoming, read, counters) {
  const before = held?.task;
  const olderContent = !counters.currentWatchCounters && Date.parse(incoming.updated_at) < Date.parse(before?.updated_at);
  const stronger = strongerMarks(before, incoming);
  const stale = olderContent && stronger || uncertainCounters(held, incoming, read, counters, olderContent);
  const source = markerSource(before, incoming, olderContent, counters.staleWatchCounters);
  return { source: stronger ? source : before, stale: Boolean(stale) };
}

function markerSource(before, incoming, olderContent, staleWatchCounters) {
  if (!olderContent) return incoming;
  const source = { ...before, ...Object.fromEntries(markerFields.map((field) => [field, incoming[field]])) };
  // Keeping the newer body must not resurrect counter families already made
  // unknown by the older observation's advancing read floor.
  return staleWatchCounters ? retainedWatchObservation(source, before) : source;
}

const uncertainCounters = (held, incoming, read, counters, olderContent) => counters.staleWatchCounters
  || countersOvertaken(held, incoming, read, counters.currentWatchCounters)
  || needsCounterRefresh(held, incoming, olderContent, counters) || incompleteActionCounters(held, incoming, counters);

const completeCounterFields = (task, fields) => fields.every((field) => Number.isSafeInteger(task?.[field]) && task[field] >= 0);
const completeCounters = (task) => counterFamilies.every(({ fields }) => completeCounterFields(task, fields));
const needsCounterRefresh = (held, incoming, olderContent, counters) => held?.needs_unread_refresh
  && (counters.action || olderContent || !coversMarks(held.task, incoming) || !completeCounters(counters.source));

function incompleteActionCounters(held, incoming, { source, fullTask, action, fullRead }) {
  if (!action && !fullRead) return false;
  return counterFamilies.some(({ floor, fields }) => !completeCounterFields(source, fields)
    && (fullTask || compareTaskMarkers(floor(incoming), floor(held?.task)) > 0));
}

const countersOvertaken = (held, incoming, read, newWatch) => (newWatch || !olderRow(held, incoming, read))
  && counterFamilies.some(({ floor }) => compareTaskMarkers(floor(held?.task), floor(incoming)) > 0);

const olderRow = (held, task, read) => held?.read_order > read || Date.parse(held?.task?.updated_at) > Date.parse(task.updated_at);

function watchAuthorityRead(held, task, read, counters) {
  if (typeof task.watched !== "boolean") return null;
  const issued = counters.confirmedAction ? counters.issuedRead : counters.action ? null : read;
  return issued != null && issued >= (held?.watch_read_order ?? held?.read_order ?? 0) ? issued : null;
}

const staleWatchObservation = (authority, task, counters) => authority === null
  && (counters.confirmedAction || !counters.action && typeof task.watched === "boolean");

const advancingCounterFloor = (before, task) => counterFamilies.some(({ floor }) => compareTaskMarkers(floor(task), floor(before)) > 0);

function retainedWatchObservation(task, held) {
  const next = withTaskWatchLineage(task, held);
  for (const { floor, fields } of counterFamilies) {
    if (compareTaskMarkers(floor(next), floor(held)) > 0) for (const field of fields) delete next[field];
  }
  return next;
}

const watchLineageBase = (held, newWatch) => newWatch
  ? { ...held, task: withoutWatchCounters(held?.task || {}), needs_unread_refresh: false } : held;

function watchReconciliation(held, task, read, counters) {
  const authority = watchAuthorityRead(held, task, read, counters);
  const changed = changedWatchLineage(held?.task, task);
  const newWatch = authority !== null && changed;
  const current = newWatch ? withTaskWatchLineage(task, counters.source) : task;
  const staleWatch = staleWatchObservation(authority, task, counters);
  const incoming = staleWatch ? retainedWatchObservation(task, held?.task) : current;
  const base = watchLineageBase(held, newWatch);
  return { authority, incoming, base, newWatch,
    staleWatchCounters: staleWatch && advancingCounterFloor(held?.task, incoming) };
}

function reviewedWatchTask(held, task, read, watch, currentWatchCounters) {
  const before = held?.task;
  const older = olderRow(held, task, read);
  const reviewed = preserveTaskReviewSummary(before, task, {
    readOrder: read, keepHeld: older, allowMissing: !older, baseRow: older ? before : task,
  });
  // Current full watch headers have their own dispatch authority, independent
  // of the ordinary body's timestamp. Earlier issued pages retain the latest
  // lineage; confirmed actions also check the joint per-task action note.
  return currentWatchCounters ? withTaskWatchLineage(reviewed, task)
    : watch.authority !== null ? { ...reviewed, watched: task.watched, watch_started_after: task.watch_started_after } : reviewed;
}

function summaryValue(held, task, read, counters = { source: task, fullTask: false, action: false }) {
  const watch = watchReconciliation(held, task, read, counters);
  task = watch.incoming;
  const currentWatchCounters = watch.newWatch || watch.authority !== null && !counters.action;
  const confirmed = reviewedWatchTask(held, task, read, watch, currentWatchCounters);
  // Marks and their paired counters have their own authority, independently
  // of ordinary row request order. Preserve held fields while accepting a
  // stronger incoming mark, including an older dispatch's read report.
  const authority = markerAuthority(watch.base, task, read, { ...counters, currentWatchCounters, staleWatchCounters: watch.staleWatchCounters });
  const options = { watchAuthority: watch.authority !== null };
  const next = preserveTaskActionMarks(authority.source, preserveTaskActionMarks(watch.base?.task, confirmed, options), options);
  return { task: next, read_order: Math.max(read, held?.read_order || 0),
    watch_read_order: watch.authority ?? held?.watch_read_order,
    needs_unread_refresh: next.watched !== false && authority.stale };
}

const summaryContentBaseline = (held, barrier) => held?.task
  || (barrier && withTaskContentComponents(summaryPatch(undefined, barrier, true), barrier, "summary"));

function summaryContent(held, row, source, { context, address, written, contentRead, headerBarrier, headerGuards, headerSource, observed = true } = {}) {
  const baseline = summaryContentBaseline(held, headerBarrier);
  const projected = headerSource ?? summaryPatch(baseline, source, true);
  if (source?.__task_content_cache) projected.__task_content_cache = source.__task_content_cache;
  const candidate = observed ? observeTaskContent(projected, { contentRead, coverage: "summary" }) : projected;
  const selected = reconcileTaskContentWrite(baseline, candidate, { context, address, heldWritten: written, headerBarrier, headerGuards });
  const task = selected.task || baseline;
  return { row: { ...row, task: task ? withTaskContentComponents(row.task, task, "summary") : null, contentAccepted: selected.acceptedHeader },
    retire: selected.retired };
}

/** Commit membership/cursor, summaries and shared identities together. Failure
 * leaves the previous cursor usable. Empty continuation pages are successful
 * pages; duplicate/late replies never consume another page's cursor.
 * `accept` checks the captured session/account lifetime inside the transaction. */
export async function landSummaryPage(ticket, page, { read, accept = () => true }) {
  validatePage(ticket, page);
  if (!Number.isSafeInteger(read) || read <= 0) throw new Error("Invalid summary read order");
  const tasks = [...new Map(page.tasks.map((task) => [task.id, task])).values()];
  const identities = Object.entries(page.identities || {});
  const addresses = [ticket.address, invalidationAddress(ticket.scope), observationAddress(ticket.scope), ...tasks.map((task) => summaryAddress(ticket.scope, task.id)),
    ...identities.map(([id]) => identityAddress(ticket.scope, id))];
  return mergeTaskContentRecordsTogether(ticket.scope, addresses, ([query, invalidation, observation, ...records], context) => {
    const held = query?.value;
    if (!acceptsPage(held, ticket, accept) || invalidatedTicket(invalidation?.value, ticket)) return null;
    if (changedContinuationBasis(held, ticket, page)) return null;
    const rows = tasks.map((task, index) => pageRow(records[index]?.value, task, read, page.basis, { context, address: summaryAddress(ticket.scope, task.id),
      written: recordWriteOf(records[index]), contentRead: ticket.contentRead }));
    if (rows.some((row) => !row.task?.id)) return null;
    const actors = identities.map(([, identity], index) => records[tasks.length + index]?.value?.read_order > read
      ? null : { identity, read_order: read });
    return { values: [membership(held, ticket, page, tasks.map((task) => task.id), identities.map(([id]) => id), read), null,
      newerObservation(observation?.value, read, page.basis), ...rows, ...actors] };
  });
}

/** Immediate action result or optimistic patch. The shared row and project
 * invalidation commit together, fencing every older query without enumerating
 * inactive query records. Callers own rollback/confirmation of optimistic edits. */
export function writeSummaryTask(scope, task, read, options = {}) {
  const mutation = summaryTaskMutation(scope, task, read, options);
  return mergeCachedRecordsTogether(mutation.addresses, mutation.reduce);
}

/** A bounded transaction fragment for action owners updating other replicas.
 * The reducer receives whole cache records and returns values, exactly like
 * mergeCachedRecordsTogether. Compose it in that same transaction; a refused
 * reducer must refuse the entire action. No query membership is manufactured,
 * and an action acknowledgment never supplies a projection basis.
 * fullTask explicitly selects authoritative omissions; compact patches retain
 * fields they omit. Both paths use the same PR, marker and counter authority. */
const fullHeaderSource = (task, fullTask) => fullTask ? fullTaskSummary(task) : undefined;
const fullActionSource = (task) => ({ read_through: null, watch_started_after: null, dismissed_through: null, ...task });

function reduceSummaryMutation(records, options, { headerBarrier, headerGuards }) {
  const [held, invalidation, ...shared] = records, { task, read, fullTask, fragment, address } = options;
  if (!options.accept()) return null;
  const source = fullTask ? fullActionSource(task) : task;
  const before = held?.value;
  const patched = summaryPatch(before?.task, source, fullTask);
  if (task.__task_content_cache) patched.__task_content_cache = task.__task_content_cache;
  const row = summaryValue(before, patched, read, { ...options, source, action: true });
  const selected = summaryContent(before, row, patched, { context: fragment.context(shared), address,
    written: recordWriteOf(held), contentRead: options.contentRead, headerBarrier, headerGuards, observed: options.rawObservation || Boolean(options.contentRead),
    headerSource: fullHeaderSource(task, fullTask) });
  return [selected.row, { read_order: Math.max(read, invalidation?.value?.read_order || 0) },
    ...fragment.finish(shared, { retire: selected.retire })];
}

export function summaryTaskMutation(scope, task, read, { accept = () => true, fullTask = false, confirmedAction = false, issuedRead, contentRead, rawObservation = false } = {}) {
  validateSummaryMutation(scope, task, read, fullTask, confirmedAction);
  const fragment = taskContentFragment(scope), address = summaryAddress(scope, task.id);
  const options = { task, read, accept, fullTask, confirmedAction, issuedRead, contentRead, rawObservation, fragment, address };
  return { addresses: [address, invalidationAddress(scope), ...fragment.addresses],
    context: (records) => fragment.context(records.slice(2)),
    reduce: (records, guards = {}) => reduceSummaryMutation(records, options, guards) };
}

// Full reads share the independent field reducers with pages, but carry no
// mutation note or projection basis. The caller supplies the shared transaction
// context and publishes this row before selecting its full detail components.
export function summaryTaskReadMutation(scope, task, contentRead) {
  validateSummaryMutation(scope, task, contentRead.readOrder, true, false);
  const address = summaryAddress(scope, task.id);
  return { addresses: [address, invalidationAddress(scope)],
    reduce: (records, options) => reduceSummaryRead(records, { ...options, task, contentRead, address }) };
}

function reduceSummaryRead([held, invalidation], { task, contentRead, ordinaryRead = contentRead, address, context, headerBarrier }) {
  const before = held?.value, source = fullActionSource(task);
  const patched = summaryPatch(before?.task, source, true), read = contentRead.readOrder;
  const row = summaryValue(before, patched, read, { source, fullTask: true, fullRead: true, action: false });
  const selected = summaryContent(before, row, patched, { context, address, written: recordWriteOf(held),
    contentRead: ordinaryRead, headerBarrier, headerSource: fullTaskSummary(task) });
  if (!selected.row.task) return null;
  return readObservationValues(before, selected.row, invalidation?.value, read);
}

const compactSemantics = (task) => JSON.stringify(summaryPatch(undefined, task || {}));
function readObservationValues(before, row, invalidation, read) {
  const changed = compactSemantics(before?.task) !== compactSemantics(row.task);
  const uncertain = Boolean(before?.basis_uncertain || changed);
  return [{ ...row, basis: uncertain ? undefined : before?.basis, basis_uncertain: uncertain },
    changed ? { read_order: Math.max(read, invalidation?.read_order || 0) } : null];
}

function validateSummaryMutation(scope, task, read, fullTask, confirmedAction) {
  if (typeof task?.id !== "string" || !task.id || !Number.isSafeInteger(read) || read <= 0) throw new Error("Invalid summary write");
  if (typeof fullTask !== "boolean" || typeof confirmedAction !== "boolean") throw new Error("Expected explicit task mutation authority flags");
  if (Object.hasOwn(task, "project_id") && task.project_id !== scope.projectId) throw new Error("Foreign project summary write");
}

/** Cache-only reconstruction. Loaded membership never claims a project total
 * and an absent entity is never inferred to be deleted by a partial query. */
const rowOvertaken = (held, row, read) => row?.needs_unread_refresh || row?.basis_uncertain || differentBasis(held, row)
  || (row?.read_order || 0) > read && !unchangedBasis(held, row);
const differentBasis = (held, row) => Boolean(held.basis && row?.basis && held.basis !== row.basis);
const rowsOvertaken = (held, records) => held.ids.some((id, index) => rowOvertaken(held, records[index]?.value, held.row_reads?.[id] || 0));

export async function readSummaryQuery(scope, filters) {
  const record = await readCached(summaryQueryAddress(scope, filters));
  if (!record?.value) return null;
  const held = record.value;
  const ids = held.identity_ids || [];
  const [invalidation, observation, ...records] = await readCachedMany([
    invalidationAddress(scope), observationAddress(scope), ...held.ids.map((id) => summaryAddress(scope, id)), ...ids.map((id) => identityAddress(scope, id)),
  ]);
  const missingIds = held.ids.filter((_id, index) => !records[index]?.value?.task);
  return { ...held,
    complete: held.complete && missingIds.length === 0,
    missing_ids: missingIds,
    stale: missingIds.length > 0 || queryOvertaken(held, invalidation?.value, observation?.value) || rowsOvertaken(held, records),
    tasks: records.slice(0, held.ids.length).flatMap((record, index) => record?.value?.task
      ? [{ ...record.value.task, ...held.row_fields?.[held.ids[index]] }] : []),
    identities: Object.fromEntries(ids.flatMap((id, index) => {
      const value = records[held.ids.length + index]?.value?.identity;
      return value ? [[id, value]] : [];
    })),
  };
}

const queryOvertaken = (held, invalidation, observation) => invalidatedQuery(held, invalidation)
  || observedNewBasis(held, observation);
const invalidatedQuery = (held, invalidation) => (invalidation?.read_order || 0) > (held.published_generation || 0);
const observedNewBasis = (held, observation) => (observation?.read_order || 0) > (held.observed_read_order || 0)
  && !unchangedBasis(held, observation);
