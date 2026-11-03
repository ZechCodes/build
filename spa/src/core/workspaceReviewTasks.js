// Request-review task choices keep the project catalogue separate from their
// bounded query pages. Every option is read back from cache after a pull.
import { cachedAddresses, readCachedMany } from "./localCache.js";
import { TRACKER_TASK_KIND, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND } from "./trackerCache.js";
import { TASK_SUMMARY_KIND, beginSummaryQuery, landSummaryPage, readSummaryQuery, summaryQueryAddress } from "./taskSummaryCache.js";
import { canonicalSummaryFilters } from "./taskSummaryQuery.js";
import { captureTaskContentRead, TASK_CONTENT_CACHE } from "./taskContentAuthority.js";
import { taskMutationEpoch, taskMutationsPending } from "./taskMutationFence.js";

const listKinds = new Set([TRACKER_TASK_KIND, TRACKER_TASKS_KIND, TRACKER_TASKS_QUERY_KIND, TASK_SUMMARY_KIND, "task-review"]);
const eligible = (task, projectId) => task.state === "open" && !task.review_summary && (!task.project_id || task.project_id === projectId);
export const reviewTaskQuery = (scope) => ({ project_id: scope.projectId, summary: true, state: "open" });
const linkedQuery = (scope) => ({ ...reviewTaskQuery(scope), linked_workspace_id: scope.workspaceId });
const filtersOf = ({ project_id: _project, summary: _summary, ...filters }) => canonicalSummaryFilters(filters);
const incomplete = () => ({ complete: false, blocked: true });

function newerChoice(held, task, normalized, read) {
  if (read !== held.read) return read > held.read;
  if (normalized !== held.normalized) return normalized;
  // Migrated legacy-only records have no issued order. Retain dev's
  // fallback there; it never outranks an admitted normalized header.
  return normalized || read > 0 || (Date.parse(task.updated_at) || 0) >= (Date.parse(held.task.updated_at) || 0);
}

function selectChoice(tasks, task, normalized) {
  const read = task[TASK_CONTENT_CACHE]?.header?.read_order || 0;
  const held = tasks.get(task.id);
  if (!held || newerChoice(held, task, normalized, read)) {
    tasks.set(task.id, { task, read, normalized });
  }
}

function linkedChoice(source, linked) {
  const row = linked?.tasks.find((task) => task.id === source.task.id);
  return row && linked.row_reads?.[row.id] === source.read
    ? { ...source.task, matched_memberships: row.matched_memberships } : source.task;
}

/** Prefer admitted header read orders, then filter eligibility. Legacy-only
 * cached options remain available before a normalized page has been loaded. */
export async function readReviewTaskChoices(scope) {
  const addresses = (await cachedAddresses({ deviceId: scope.deviceId, entityId: scope.projectId })).filter((address) => listKinds.has(address.kind));
  const [records, linked] = await Promise.all([readCachedMany(addresses),
    scope.workspaceId ? readSummaryQuery(scope, filtersOf(linkedQuery(scope))) : null]);
  const tasks = new Map();
  const sources = records.map((record, index) => record && { ...record, address: addresses[index] });
  for (const record of sources.filter(Boolean).sort((a, b) => a.at - b.at)) {
    const rows = record.value?.task ? [record.value.task] : record.value?.tasks || [];
    for (const task of rows) selectChoice(tasks, task, record.address.kind === TASK_SUMMARY_KIND);
  }
  const reviews = new Set(records.filter((record) => record?.value?.review).map((record) => record.value.review.task_id));
  return [...tasks.values()].map((source) => linkedChoice(source, linked))
    .filter((task) => !reviews.has(task.id) && eligible(task, scope.projectId)).sort((a, b) => b.number - a.number);
}

export function preselectedReviewTask(context, tasks) {
  const workspaceId = context.workspace?.workspace_id || context.workspace?.id;
  return tasks.find((task) => task.links?.workspace_ids?.includes(workspaceId) || task.matched_memberships?.linked_workspace_id === workspaceId)?.id || "";
}

/** The loaded normalized query, including its cursor and stale/complete flags. */
export async function readReviewTaskPage(scope) {
  return readSummaryQuery(scope, filtersOf(reviewTaskQuery(scope)));
}

async function pullChoicePage(scope, params, cursor, active) {
  const { contextFor, deviceContextIdentity, whenGreeted } = await import("./deviceContexts.js");
  const { deviceSession } = await import("./deviceReconnect.js");
  const context = contextFor(scope.deviceId);
  const identity = deviceContextIdentity(scope.deviceId), session = deviceSession(scope.deviceId);
  const epoch = taskMutationEpoch(scope.deviceId);
  const accept = () => active() && contextFor(scope.deviceId) === context
    && deviceContextIdentity(scope.deviceId) === identity && deviceSession(scope.deviceId) === session
    && taskMutationEpoch(scope.deviceId) === epoch && taskMutationsPending(scope.deviceId) === 0;
  if (!context) return publishChoicePage(scope, params, cursor, accept);
  const request = await whenGreeted(context, (stands) => {
    const support = context.adapter?.capabilities?.reviews;
    if (!support?.taskChoices || params.linked_workspace_id && !support.linkedTaskChoices) throw new Error("Update the bridge to choose review tasks.");
    return publishChoicePage(scope, params, cursor, () => accept() && context.active() && stands());
  });
  return request ? request.sent : incomplete();
}

async function choiceTicket(scope, filters, cursor, contentRead, accept) {
  if (!cursor) return beginSummaryQuery(scope, filters, contentRead.readOrder, { contentRead, accept });
  const held = await readSummaryQuery(scope, filters);
  if (!held || held.refreshing || held.complete || held.next_cursor !== cursor) return null;
  if (held.stale && held.snapshot_continuation_eligible !== true) return null;
  return { scope, filters, address: summaryQueryAddress(scope, filters), generation: held.generation, cursor, contentRead };
}

async function publishedChoicePage(scope, filters, ticket, read) {
  const page = await readSummaryQuery(scope, filters);
  if (!page || page.refreshing || page.generation !== ticket.generation || page.observed_read_order !== read) return incomplete();
  return { complete: true, page };
}

async function publishChoicePage(scope, params, cursor, accept) {
  if (!accept()) return incomplete();
  const contentRead = await captureTaskContentRead(scope, { revalidate: "summary" });
  const filters = filtersOf(params);
  const ticket = await choiceTicket(scope, filters, cursor, contentRead, accept);
  if (!ticket || !accept()) return incomplete();
  const answer = await scope.callRpc("tasks.list", { ...params, limit: 100, ...(cursor ? { cursor } : {}) });
  if (!accept()) return incomplete();
  if (cursor && answer.next_cursor === cursor) return { ...incomplete(), stalled: true };
  const complete = await landSummaryPage(ticket, answer, { read: contentRead.readOrder, accept });
  return complete ? publishedChoicePage(scope, filters, ticket, contentRead.readOrder) : incomplete();
}

async function refreshLinkedChoices(scope, active, changed) {
  let cursor = null;
  do {
    const outcome = await pullChoicePage(scope, linkedQuery(scope), cursor, active);
    if (!outcome.complete && !outcome.stalled) return outcome;
    await changed();
    const next = outcome.stalled ? cursor : outcome.page.next_cursor;
    if (next === cursor) break;
    cursor = next;
  } while (cursor && active());
  return { complete: true };
}

/** Linked pages retain automatic continuation; ordinary pages remain demand
 * driven. complete reports admitted publication, not exhausted membership.
 * Refused owners return incomplete; RPC/protocol failures still reject. */
export async function refreshReviewTaskChoices(scope, support, active, changed, cursor = null) {
  if (!support.taskChoices) return incomplete();
  if (!cursor && support.linkedTaskChoices) {
    const linked = await refreshLinkedChoices(scope, active, changed);
    if (!linked.complete) return linked;
  }
  if (!active()) return incomplete();
  const outcome = await pullChoicePage(scope, reviewTaskQuery(scope), cursor, active);
  if (outcome.complete && active()) await changed();
  return outcome;
}
