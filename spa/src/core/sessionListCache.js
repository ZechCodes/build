import { mergeCachedAtomically, mergeCachedTogether, readCached } from "./localCache.js";

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
    [rowId(kind, row), { ...row.conversation_session_revisions }]));
}

const resetSinceObservation = (held, observed) => observed !== undefined
  && Object.entries(held?.conversation_session_revisions || {}).some(([id, revision]) => Number(revision) > Number(observed[id] || 0));

function listedSession(incoming, held, observation) {
  const next = monotonicSession(incoming, held);
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
      ? monotonicSession({ ...row, ...incoming }, row)
      : row);
    return present ? next : [...next, incoming];
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
