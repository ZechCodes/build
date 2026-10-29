// Normalized summaries and independent loaded query membership. This module
// is a cache writer/reader only: it neither asks the wire nor drains cursors.
// Detail/timeline records belong to trackerCache and are never replaced here.
import { mergeCachedRecordsTogether, readCached, readCachedMany } from "./localCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { compareTaskMarkers, isPresentTaskMarker, newerTaskMarker } from "./taskOpaqueMarkers.js";
import { preserveTaskReviewSummary } from "./trackerCache.js";
import { summaryPatch } from "./taskSummaryProjection.js";

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
export async function beginSummaryQuery(scope, filters, generation, { accept = () => true } = {}) {
  if (!Number.isSafeInteger(generation) || generation <= 0) throw new Error("Invalid summary query generation");
  const queryAddress = summaryQueryAddress(scope, filters);
  await mergeCachedRecordsTogether([queryAddress], ([held]) => {
    if (!accept() || held?.value?.generation >= generation) return null;
    return [{ ids: [], identity_ids: [], next_cursor: null, complete: false,
      ...held?.value, generation, refreshing: true }];
  });
  return { scope: { ...scope }, address: queryAddress, filters: canonicalSummaryFilters(filters), generation, cursor: null };
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

function pageRow(held, task, read, basis) {
  const incoming = entityFields(task), older = olderRow(held, incoming, read);
  const value = summaryValue(held, incoming, read);
  const source = older ? held?.task : incoming;
  const mixed = changedBasisAuthority(held, basis, source, value.task);
  const uncertain = mixed || older && held?.basis_uncertain === true;
  return { ...value, basis: uncertain ? undefined : older ? held?.basis : basis, basis_uncertain: uncertain };
}

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

function preserveMarks(held, task) {
  const next = { ...task };
  for (const field of ["read_through", "watch_started_after", "dismissed_through"]) {
    if (field === "watch_started_after" && next.watched === false) continue;
    const mark = newerTaskMarker(held?.[field], next[field]);
    if (isPresentTaskMarker(mark)) next[field] = mark;
  }
  if (next.watched === false) return withoutWatchCounters(next);
  for (const { floor, fields } of counterFamilies) preserveCounterFamily(next, held, task, floor, fields);
  return next;
}

function withoutWatchCounters(task) {
  const next = { ...task };
  for (const key of ["watch_started_after", "unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) delete next[key];
  return next;
}

const counterFloor = (task) => newerTaskMarker(task?.read_through, task?.watch_started_after);
const counterFamilies = [
  { floor: counterFloor, fields: ["unread_count"] },
  { floor: (task) => task?.read_through, fields: ["unread_mentions", "unread_agent_comments"] },
];

function preserveCounterFamily(next, held, task, floor, fields) {
  if (compareTaskMarkers(floor(held), floor(task)) <= 0) return;
  for (const field of fields) if (Object.hasOwn(held, field)) next[field] = held[field];
}

const markerFields = ["read_through", "watch_started_after", "dismissed_through"];
const strongerMarks = (held, incoming) => markerFields.some((field) => compareTaskMarkers(incoming[field], held?.[field]) > 0);
const coversMarks = (held, incoming) => markerFields.every((field) =>
  compareTaskMarkers(incoming[field], held?.[field]) >= 0);

function markerAuthority(held, incoming, read) {
  const before = held?.task;
  const olderContent = Date.parse(incoming.updated_at) < Date.parse(before?.updated_at);
  const stronger = strongerMarks(before, incoming);
  const stale = olderContent && stronger || countersOvertaken(held, incoming, read)
    || needsCounterRefresh(held, incoming, olderContent);
  const source = olderContent ? Object.fromEntries(markerFields.map((field) => [field, incoming[field]])) : incoming;
  return { source: stronger ? source : before, stale: Boolean(stale) };
}

const needsCounterRefresh = (held, incoming, olderContent) => held?.needs_unread_refresh
  && (olderContent || !coversMarks(held.task, incoming));

const countersOvertaken = (held, incoming, read) => !olderRow(held, incoming, read)
  && counterFamilies.some(({ floor }) => compareTaskMarkers(floor(held?.task), floor(incoming)) > 0);

const olderRow = (held, task, read) => held?.read_order > read || Date.parse(held?.task?.updated_at) > Date.parse(task.updated_at);

function summaryValue(held, task, read) {
  const before = held?.task;
  const older = olderRow(held, task, read);
  const reviewed = preserveTaskReviewSummary(before, task, {
    readOrder: read, keepHeld: older, allowMissing: !older, baseRow: older ? before : task,
  });
  // Marks and their paired counters have their own authority, independently
  // of ordinary row request order. Preserve held fields while accepting a
  // stronger incoming mark, including an older dispatch's read report.
  const authority = markerAuthority(held, task, read);
  const next = preserveMarks(authority.source, preserveMarks(before, reviewed));
  return { task: next, read_order: Math.max(read, held?.read_order || 0), needs_unread_refresh: next.watched !== false && authority.stale };
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
  return mergeCachedRecordsTogether(addresses, ([query, invalidation, observation, ...records]) => {
    const held = query?.value;
    if (!acceptsPage(held, ticket, accept) || invalidatedTicket(invalidation?.value, ticket)) return null;
    if (changedContinuationBasis(held, ticket, page)) return null;
    const rows = tasks.map((task, index) => pageRow(records[index]?.value, task, read, page.basis));
    const actors = identities.map(([, identity], index) => records[tasks.length + index]?.value?.read_order > read
      ? null : { identity, read_order: read });
    return [membership(held, ticket, page, tasks.map((task) => task.id), identities.map(([id]) => id), read), null,
      newerObservation(observation?.value, read, page.basis), ...rows, ...actors];
  });
}

/** Immediate action result or optimistic patch. The shared row and project
 * invalidation commit together, fencing every older query without enumerating
 * inactive query records. Callers own rollback/confirmation of optimistic edits. */
export function writeSummaryTask(scope, task, read, { accept = () => true, fullTask = false } = {}) {
  if (!task?.id || !Number.isSafeInteger(read) || read <= 0) throw new Error("Invalid summary write");
  return mergeCachedRecordsTogether([summaryAddress(scope, task.id), invalidationAddress(scope)], ([held, invalidation]) => {
    if (!accept()) return null;
    const row = summaryValue(held?.value, summaryPatch(held?.value?.task, task, fullTask), read);
    if (!row) return null;
    return [row, { read_order: Math.max(read, invalidation?.value?.read_order || 0) }];
  });
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
