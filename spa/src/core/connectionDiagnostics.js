// What this tab has seen its connections do, since the page loaded.
//
// # Why the ring is large and says when it overflowed
//
// The maintainer reported a phone "spazzing" and pasted their diagnostics; they
// covered only the final session — the one that worked. The three failures
// before it, which were the entire story, had been pushed out of a hundred-entry
// ring by the very reconnect storm they caused, so the report carried the
// recovery and none of the cause (task #60). A ring that silently drops the interesting part is worse
// than no ring, because it reads like a complete account.
//
// So: room for a wake's worth of events, and when it does overflow the report
// says how many it lost rather than presenting a truncated history as a whole
// one. Every entry already carries the connection it belongs to, minted as
// `<deviceId>:<sessionId>`, which is what lets a reader follow one session
// through a storm of four.

/** How many events this tab keeps.
 *
 *  A single reconnect writes a dozen or more — negotiating, ICE states, channel
 *  opens, candidate failures, the carrying verdict — and the storm behind #60 was
 *  four sessions inside ninety seconds. A hundred could not hold one wake; this
 *  holds many, and costs a few hundred kilobytes at worst on a page that has been
 *  open all day. */
export const DIAGNOSTIC_LIMIT = 2000;

const history = [];
/** When this tab started recording, so a reader can tell "nothing happened" from
 *  "this page has only just opened". */
const since = Date.now();
/** How many events were dropped to make room. Reported rather than hidden: a
 *  reader who cannot see it would read a truncated history as the whole one. */
let dropped = 0;

export function recordConnectionDiagnostic(connection, event, detail = {}) {
  history.push({ at: Date.now(), connection, event, ...detail });
  if (history.length > DIAGNOSTIC_LIMIT) {
    dropped += history.length - DIAGNOSTIC_LIMIT;
    history.splice(0, history.length - DIAGNOSTIC_LIMIT);
  }
}

/** The events alone, oldest first — what the panel renders and what
 *  `buildConnectionDiagnostics()` has always answered (deploy/OPS.md). */
export function connectionDiagnosticHistory() {
  return history.map((entry) => ({ ...entry }));
}

/** The whole record: when this tab started, what it lost, and everything it
 *  still holds. What gets copied and shared, because a report that cannot say it
 *  is incomplete is the failure this exists to fix. */
export function connectionDiagnosticReport() {
  return { since, dropped, events: connectionDiagnosticHistory() };
}

export function clearConnectionDiagnosticHistory() {
  history.length = 0;
  dropped = 0;
}
