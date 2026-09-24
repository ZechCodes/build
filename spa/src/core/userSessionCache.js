// The user's session on one bridge, as `issues.list` carries it.
//
// Device-wide rather than per project: the bridge keeps one summary of what
// the user did, and every project's list answers with the same one. So it is
// held once per device, and whichever list read lands last writes it.

import { mergeCachedAtomically, readCached } from "./localCache.js";

export const USER_SESSION_KIND = "user-session";

export const userSessionAddress = (deviceId) => ({ deviceId, entityId: "", kind: USER_SESSION_KIND });

const instant = (value) => (Number.isFinite(value) ? value : null);

/** The session an `issues.list` answer carries, or null from a bridge that
 *  sends none. */
export function userSessionOf(answer) {
  const session = answer?.user_session;
  if (!session || typeof session !== "object" || !Number.isFinite(session.gap_ms)) return null;
  return {
    session_started_ms: instant(session.session_started_ms),
    last_activity_ms: instant(session.last_activity_ms),
    previous_session_ended_ms: instant(session.previous_session_ended_ms),
    gap_ms: session.gap_ms,
  };
}

const sameSession = (a, b) =>
  a.session_started_ms === b.session_started_ms
  && a.last_activity_ms === b.last_activity_ms
  && a.previous_session_ended_ms === b.previous_session_ended_ms
  && a.gap_ms === b.gap_ms;

/** Write what one list answer carried. Two reads can land out of order, and
 *  the user only ever moves forward, so an answer older than the held one
 *  (by its last activity) is dropped, as is one that changes nothing. */
export function writeUserSession(deviceId, answer) {
  const session = userSessionOf(answer);
  if (!session) return Promise.resolve(false);
  return mergeCachedAtomically(userSessionAddress(deviceId), (held) => {
    if (held && sameSession(held, session)) return null;
    if ((held?.last_activity_ms ?? -Infinity) > (session.last_activity_ms ?? -Infinity)) return null;
    return session;
  });
}

export async function readUserSession(deviceId) {
  const record = await readCached(userSessionAddress(deviceId));
  return record?.value || null;
}
