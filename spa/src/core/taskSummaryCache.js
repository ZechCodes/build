// Normalized summaries and independent loaded query membership. This module
// is a cache writer/reader only: it neither asks the wire nor drains cursors.
// Detail/timeline records belong to trackerCache and are never replaced here.
import { mergeCachedRecordsTogether, readCached, readCachedMany } from "./localCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { latestTaskMark } from "./trackerUnread.js";
import { preserveTaskReviewSummary } from "./trackerCache.js";
import { summaryPatch } from "./taskSummaryProjection.js";

export const TASK_SUMMARY_KIND = "tracker-summary";
export const TASK_SUMMARY_QUERY_KIND = "tracker-summary-query";
export const TASK_SUMMARY_IDENTITY_KIND = "tracker-summary-identity";

const address = ({ deviceId, projectId }, kind, sub) => ({ deviceId, entityId: String(projectId), kind, sub });
export const summaryAddress = (scope, id) => address(scope, TASK_SUMMARY_KIND, id);
const identityAddress = (scope, id) => address(scope, TASK_SUMMARY_IDENTITY_KIND, id);
const invalidationAddress = (scope) => address(scope, "tracker-summary-invalidation", "");
export const summaryQueryAddress = (scope, filters) => address(scope, TASK_SUMMARY_QUERY_KIND,
  JSON.stringify(canonicalSummaryFilters(filters)));

/** Start a refresh without discarding the last successful cached paint.
 * A caller takes generation from nextTaskRead before dispatch. Older tickets
 * cannot reset a newer refresh, including across tabs. */
export async function beginSummaryQuery(scope, filters, generation) {
  if (!Number.isSafeInteger(generation) || generation <= 0) throw new Error("Invalid summary query generation");
  const queryAddress = summaryQueryAddress(scope, filters);
  await mergeCachedRecordsTogether([queryAddress], ([held]) => {
    if (held?.value?.generation >= generation) return null;
    return [{ ids: [], identity_ids: [], next_cursor: null, complete: false,
      ...held?.value, generation, refreshing: true }];
  });
  return { scope: { ...scope }, address: queryAddress, generation, cursor: null };
}

function validatePage(ticket, page) {
  if (page?.summary !== true || !Array.isArray(page.tasks)) throw new Error("Expected a task summary page");
  if (page.tasks.some((task) => typeof task?.id !== "string" || !task.id)) throw new Error("Invalid task summary identity");
  const next = page.next_cursor;
  if (next != null && (typeof next !== "string" || !next || next === ticket.cursor)) throw new Error("Invalid summary continuation");
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
    next_cursor: page.next_cursor ?? null,
    complete: page.next_cursor == null,
    refreshing: false,
    published_generation: ticket.generation,
  };
}

function preserveMarks(held, task) {
  const next = { ...task };
  let behind = false;
  for (const field of ["read_through", "watch_started_after", "dismissed_through"]) {
    const mark = latestTaskMark(held?.[field], next[field]);
    if (mark && mark !== next[field]) behind = true;
    if (mark) next[field] = mark;
  }
  if (behind) for (const field of ["unread_count", "unread_mentions", "unread_agent_comments"]) {
    if (Object.hasOwn(held, field)) next[field] = held[field];
  }
  return next;
}

function summaryValue(held, task, read) {
  const older = held?.read_order > read || Date.parse(held?.task?.updated_at) > Date.parse(task.updated_at);
  const next = preserveMarks(held?.task, preserveTaskReviewSummary(held?.task, task, {
    readOrder: read, keepHeld: older, allowMissing: !older, baseRow: older ? held.task : task,
  }));
  return { task: next, read_order: Math.max(read, held?.read_order || 0) };
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
  const addresses = [ticket.address, invalidationAddress(ticket.scope), ...tasks.map((task) => summaryAddress(ticket.scope, task.id)),
    ...identities.map(([id]) => identityAddress(ticket.scope, id))];
  return mergeCachedRecordsTogether(addresses, ([query, invalidation, ...records]) => {
    const held = query?.value;
    if (!acceptsPage(held, ticket, accept) || invalidation?.value?.read_order > ticket.generation) return null;
    const rows = tasks.map((task, index) => summaryValue(records[index]?.value, task, read));
    const actors = identities.map(([, identity], index) => records[tasks.length + index]?.value?.read_order > read
      ? null : { identity, read_order: read });
    return [membership(held, ticket, page, tasks.map((task) => task.id), identities.map(([id]) => id), read), null, ...rows, ...actors];
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
const rowsOvertaken = (held, records) => held.ids.some((id, index) =>
  (records[index]?.value?.read_order || 0) > (held.row_reads?.[id] || 0));

export async function readSummaryQuery(scope, filters) {
  const record = await readCached(summaryQueryAddress(scope, filters));
  if (!record?.value) return null;
  const held = record.value;
  const ids = held.identity_ids || [];
  const [invalidation, ...records] = await readCachedMany([
    invalidationAddress(scope), ...held.ids.map((id) => summaryAddress(scope, id)), ...ids.map((id) => identityAddress(scope, id)),
  ]);
  return { ...held,
    stale: (invalidation?.value?.read_order || 0) > (held.published_generation || 0) || rowsOvertaken(held, records),
    tasks: records.slice(0, held.ids.length).map((record) => record?.value?.task).filter(Boolean),
    identities: Object.fromEntries(ids.flatMap((id, index) => {
      const value = records[held.ids.length + index]?.value?.identity;
      return value ? [[id, value]] : [];
    })),
  };
}
