import { mergeCachedAtomically } from "./localCache.js";

const rowId = (kind, row) => kind === "projects"
  ? row.project_id || row.id
  : row.workspace_id || row.id;

const validSession = (row) => Number.isSafeInteger(row?.session_started_ms)
  && Number.isSafeInteger(row?.last_activity_ms)
  && row.session_started_ms <= row.last_activity_ms;

/** Keep the summary with the latest message, even when two tabs write at once. */
export function monotonicSession(incoming, held) {
  if (!validSession(held)) return incoming;
  if (validSession(incoming) && incoming.last_activity_ms >= held.last_activity_ms) return incoming;
  return { ...incoming, session_started_ms: held.session_started_ms, last_activity_ms: held.last_activity_ms };
}

/** A bridge list is authoritative for membership. Merge every row against the
 * record held by the database while its write transaction is open. */
export function replaceSessionList(address, kind, incoming) {
  return mergeCachedAtomically(address, (held) => {
    const old = new Map((Array.isArray(held) ? held : []).map((row) => [rowId(kind, row), row]));
    return incoming.map((row) => monotonicSession(row, old.get(rowId(kind, row))));
  });
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

/** A thread tip updates an existing list row, without adding a partial row. */
export function updateSessionSummary(address, kind, id, session) {
  if (!validSession(session)) return Promise.resolve(false);
  return mergeCachedAtomically(address, (held) => {
    if (!Array.isArray(held)) return null;
    let changed = false;
    const updated = held.map((row) => {
      if (rowId(kind, row) !== id) return row;
      const merged = monotonicSession({ ...row, ...session }, row);
      if (merged.last_activity_ms === row.last_activity_ms && merged.session_started_ms === row.session_started_ms) return row;
      changed = true;
      return merged;
    });
    return changed ? updated : null;
  });
}
