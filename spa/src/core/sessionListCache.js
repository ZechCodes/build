import { mergeCachedAtomically, mergeCachedTogether, readCached } from "./localCache.js";
import { preserveReviewSummary } from "./trackerCache.js";

// A list request observes PR state beside reset revisions. The reserved key
// cannot be a conversation id and never participates in the reset comparison.
const ACTIVE_REVIEW_OBSERVATION = "__active_review";
const ACTIVE_REVIEW_CACHE = "__active_review_cache";
const reviewSummaryKey = (summary) => JSON.stringify(summary ? [summary.task_id, summary.workspace_id,
  summary.version, summary.status, summary.latest_published_snapshot_id] : null);
const validReviewSummary = (summary) => Boolean(summary?.task_id) && Number.isFinite(summary.version);
const sameReviewTask = (left, right) => validReviewSummary(left) && validReviewSummary(right) && left.task_id === right.task_id;

function activeReviewState(row) {
  const state = row?.[ACTIVE_REVIEW_CACHE];
  return { revision: Number(state?.revision) || 0, floor: state?.floor ?? row?.active_review ?? null };
}

/** Which previous PR an authoritative absence cleared. Workspace readers may
 * discover a newly opened task locally, but cannot restore this retired link. */
export const clearedWorkspaceReviewTask = (row) => row?.active_review ? null : activeReviewState(row).floor?.task_id || null;

const activeReviewObservation = (row) => [activeReviewState(row).revision, reviewSummaryKey(row?.active_review)];

const observedActiveReview = (held, observation) => observation !== undefined
  && JSON.stringify(activeReviewObservation(held)) === JSON.stringify(observation[ACTIVE_REVIEW_OBSERVATION]);
const reviewReadOvertaken = (held, observation) => observation !== undefined && !observedActiveReview(held, observation);

function reviewTombstoneBlocks(held, incoming, observation) {
  const floor = activeReviewState(held).floor;
  const summary = incoming.active_review;
  if (held?.active_review || !sameReviewTask(summary, floor)) return false;
  return summary.version < floor.version || summary.version === floor.version && !observedActiveReview(held, observation);
}

/** Retain the last PR version after authoritative absence clears the visible
 * link. Its separate revision also protects a new task in a reused workspace
 * from replies that observed the previous task or the previous absence. */
function withActiveReviewState(held, incoming) {
  const state = activeReviewState(held);
  if (!validReviewSummary(incoming.active_review) && !state.floor) return incoming;
  const changed = reviewSummaryKey(held?.active_review) !== reviewSummaryKey(incoming.active_review);
  const floor = validReviewSummary(incoming.active_review) ? incoming.active_review : state.floor;
  return { ...incoming, [ACTIVE_REVIEW_CACHE]: { revision: state.revision + Number(changed), floor } };
}

function mergedActiveReview(held, incoming, observation) {
  const next = preserveReviewSummary(held, incoming, "active_review", {
    allowMissing: observedActiveReview(held, observation),
    keepHeld: reviewReadOvertaken(held, observation) || reviewTombstoneBlocks(held, incoming, observation),
  });
  return withActiveReviewState(held, next);
}

const rowId = (kind, row) => kind === "projects"
  ? row.project_id || row.id
  : row.workspace_id || row.id;

const validSession = (row) => Number.isSafeInteger(row?.session_started_ms)
  && Number.isSafeInteger(row?.last_activity_ms)
  && row.session_started_ms <= row.last_activity_ms;

/** Keep the summary with the latest message, even when two tabs write at once. */
export function monotonicSession(incoming, held) {
  if (held?.conversation_session_revisions) incoming = { ...incoming, conversation_session_revisions: held.conversation_session_revisions };
  if (!validSession(held)) return incoming;
  if (validSession(incoming) && incoming.last_activity_ms >= held.last_activity_ms) return incoming;
  return { ...incoming, session_started_ms: held.session_started_ms, last_activity_ms: held.last_activity_ms };
}

/** A bridge list is authoritative for membership. Merge every row against the
 * record held by the database while its write transaction is open. */
export function replaceSessionList(address, kind, incoming, onReplaced, observation) {
  return mergeCachedAtomically(address, (held) => {
    onReplaced?.(held);
    const old = new Map((Array.isArray(held) ? held : []).map((row) => [rowId(kind, row), row]));
    return incoming.map((row) => listedSession(row, old.get(rowId(kind, row)), observation ? observation[rowId(kind, row)] || {} : undefined));
  });
}

/** Capture reset revisions before a list request crosses the wire. A reset
 * committed in another tab while it is out must survive the old answer. */
export async function sessionListObservation(address, kind) {
  const held = (await readCached(address))?.value;
  return Object.fromEntries((Array.isArray(held) ? held : []).map((row) =>
    [rowId(kind, row), { ...row.conversation_session_revisions, [ACTIVE_REVIEW_OBSERVATION]: activeReviewObservation(row) }]));
}

const resetSinceObservation = (held, observed) => observed !== undefined
  && Object.entries(held?.conversation_session_revisions || {}).some(([id, revision]) => Number(revision) > Number(observed[id] || 0));

function listedSession(incoming, held, observation) {
  const next = monotonicSession(mergedActiveReview(held, incoming, observation), held);
  return resetSinceObservation(held, observation)
    ? { ...next, session_started_ms: held.session_started_ms, last_activity_ms: held.last_activity_ms } : next;
}

/** A single-row answer must preserve every unrelated row in the list. */
export function upsertSessionRow(address, kind, incoming, active = () => true) {
  return mergeCachedAtomically(address, (held) => {
    if (!active()) return null;
    const rows = Array.isArray(held) ? held : [];
    const id = rowId(kind, incoming);
    const present = rows.some((row) => rowId(kind, row) === id);
    const next = rows.map((row) => rowId(kind, row) === id
      ? monotonicSession(mergedActiveReview(row, { ...row, ...incoming }), row)
      : row);
    return present ? next : [...next, mergedActiveReview(null, incoming)];
  });
}

const revisionOf = (options) => Number(options.threadGenerationRevision) || 0;
const seenRevision = (row, options) => Number(row.conversation_session_revisions?.[options.conversationId]) || 0;
const resetsSummary = (row, options) => Boolean(options.conversationId) && revisionOf(options) > seenRevision(row, options);

const sameSummary = (left, right) => left.last_activity_ms === right.last_activity_ms
  && left.session_started_ms === right.session_started_ms;

function tipSummary(row, session, options) {
  if (resetsSummary(row, options)) {
    return { ...row, session_started_ms: session?.session_started_ms ?? null, last_activity_ms: session?.last_activity_ms ?? null,
      conversation_session_revisions: { ...row.conversation_session_revisions, [options.conversationId]: revisionOf(options) } };
  }
  if (!validSession(session)) return row;
  const merged = monotonicSession({ ...row, ...session }, row);
  return sameSummary(merged, row) ? row : merged;
}

function tipSessionRows(held, kind, id, session, options) {
  if (!Array.isArray(held)) return null;
  const updated = held.map((row) => rowId(kind, row) === id ? tipSummary(row, session, options) : row);
  return updated.some((row, index) => row !== held[index]) ? updated : null;
}

const tipIsCurrent = (thread, options) => (!options.active || options.active())
  && (!options.threadId || thread?.thread_id === options.threadId);

/** A thread tip updates an existing list row, without adding a partial row.
 * A new canonical generation carries the recomputed summary, including no
 * messages; older generation tips cannot restore the erased timestamps. */
export function updateSessionSummary(address, kind, id, session, options = {}) {
  if (!validSession(session) && !revisionOf(options)) return Promise.resolve(false);
  if (!options.threadAddress) return mergeCachedAtomically(address, (held) =>
    tipIsCurrent(null, options) ? tipSessionRows(held, kind, id, session, options) : null);
  return mergeCachedTogether([options.threadAddress, address], ([thread, held]) =>
    tipIsCurrent(thread, options) ? [null, tipSessionRows(held, kind, id, session, options)] : null);
}
