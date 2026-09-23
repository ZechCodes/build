// The bridge owns session boundaries. Legacy bridges have no summary, so
// their rows enter today's inbox until a bridge with summaries answers.

export const SESSION_GAP_MS = 12 * 60 * 60 * 1000;

const emptySummary = (record) => !!record && record.session_started_ms === null
  && record?.last_activity_ms === null
  && Object.hasOwn(record, "session_started_ms")
  && Object.hasOwn(record, "last_activity_ms");

const validSummary = (record) => Number.isSafeInteger(record?.session_started_ms)
  && Number.isSafeInteger(record?.last_activity_ms)
  && record.session_started_ms <= record.last_activity_ms;

/** Read the bridge's running summary, falling back to today for older wires. */
export function sessionTimes(record, todayMs = Date.now()) {
  if (emptySummary(record)) {
    const created = Date.parse(record.created_at || "");
    const emptyMs = Number.isFinite(created) ? created : todayMs;
    return { anchorMs: emptyMs, lastActivityMs: emptyMs };
  }
  if (!validSummary(record)) return { anchorMs: todayMs, lastActivityMs: todayMs };
  return { anchorMs: record.session_started_ms, lastActivityMs: record.last_activity_ms };
}
