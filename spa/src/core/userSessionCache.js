// The user's session on one bridge, as `issues.list` and `user.present`
// carry it.
//
// Device-wide rather than per project: the bridge keeps one summary of what
// the user did, and every project's list answers with the same one. So it is
// held once per device, and whichever read lands last writes it.
//
// Beside it, the bridge's clock: the `now_ms` it answered with and when this
// device heard it. The Dashboard measures silences on the bridge's clock
// (core/trackerDashboardModel.js), and this pair is how it knows that clock
// after a reload or with nothing on the wire.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const USER_SESSION_KIND = "user-session";

export const userSessionAddress = (deviceId) => ({ deviceId, entityId: "", kind: USER_SESSION_KIND });

const instant = (value) => (Number.isFinite(value) ? value : null);

/** The session an answer carries, heard at `receivedMs` on this device's
 *  clock, or null from a bridge that sends none. */
export function userSessionOf(answer, receivedMs = Date.now()) {
  const session = answer?.user_session;
  if (!session || typeof session !== "object" || !Number.isFinite(session.gap_ms)) return null;
  return {
    session_started_ms: instant(session.session_started_ms),
    last_activity_ms: instant(session.last_activity_ms),
    previous_session_ended_ms: instant(session.previous_session_ended_ms),
    gap_ms: session.gap_ms,
    now_ms: instant(session.now_ms),
    received_ms: instant(receivedMs),
  };
}

/** How far the bridge's clock runs from this device's, as one record says. */
const skewOf = (record) => (record.now_ms ?? NaN) - (record.received_ms ?? NaN);

/** A minute: past it, a newer reading of the bridge's clock is worth a write
 *  even when the session has not moved. */
const CLOCK_DRIFT_MS = 60_000;

const sameClock = (a, b) => Math.abs(skewOf(a) - skewOf(b)) <= CLOCK_DRIFT_MS;

const sameSession = (a, b) =>
  a.session_started_ms === b.session_started_ms
  && a.last_activity_ms === b.last_activity_ms
  && a.previous_session_ended_ms === b.previous_session_ended_ms
  && a.gap_ms === b.gap_ms;

/** Write what one answer carried. Two reads can land out of order, and the
 *  user only ever moves forward, so an answer older than the held one (by its
 *  last activity) is dropped, as is one that changes nothing: the same
 *  session, and the bridge's clock where the held record already put it. */
export function writeUserSession(deviceId, answer, receivedMs = Date.now()) {
  const session = userSessionOf(answer, receivedMs);
  if (!session) return Promise.resolve(false);
  return mergeCachedAtomically(userSessionAddress(deviceId), (held) => {
    if (held && sameSession(held, session) && sameClock(held, session)) return null;
    if ((held?.last_activity_ms ?? -Infinity) > (session.last_activity_ms ?? -Infinity)) return null;
    return session;
  });
}

export async function readUserSession(deviceId) {
  const record = await readCached(userSessionAddress(deviceId));
  return record?.value || null;
}
