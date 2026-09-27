// The user's session on one bridge, as `issues.list` and `user.present`
// carry it.
//
// Device-wide rather than per project: the bridge keeps one summary of what
// the user did, and every project's list answers with the same one. So it is
// held once per device, and whichever read lands last writes it.
//
// With it, `now_ms`: the bridge's clock when it answered, which is how the
// Dashboard tells whether the user was away at that moment
// (core/trackerDashboardModel.js). Nothing here reads this device's clock.

import { deleteCached, mergeCachedAtomically, readCached } from "./localCache.js";
import { standingOf } from "./trackerDashboardModel.js";

export const USER_SESSION_KIND = "user-session";

export const userSessionAddress = (deviceId) => ({ deviceId, entityId: "", kind: USER_SESSION_KIND });

const instant = (value) => (Number.isFinite(value) ? value : null);

/** The session an answer carries, or null from a bridge that sends none. */
export function userSessionOf(answer) {
  const session = answer?.user_session;
  if (!session || typeof session !== "object" || !Number.isFinite(session.gap_ms)) return null;
  return {
    session_started_ms: instant(session.session_started_ms),
    last_activity_ms: instant(session.last_activity_ms),
    previous_session_ended_ms: instant(session.previous_session_ended_ms),
    gap_ms: session.gap_ms,
    now_ms: instant(session.now_ms),
  };
}

const sameSession = (a, b) =>
  a.session_started_ms === b.session_started_ms
  && a.last_activity_ms === b.last_activity_ms
  && a.previous_session_ended_ms === b.previous_session_ended_ms
  && a.gap_ms === b.gap_ms;

const earlier = (a, b) => (a ?? -Infinity) < (b ?? -Infinity);

/** Whether an answer says something the held record does not. Two reads can
 *  land out of order and the user only moves forward, so an answer older than
 *  the held one (by its last activity, then by when the bridge answered) is
 *  dropped. So is one that moves nothing the Dashboard reads: the same session,
 *  and the user standing where the held answer said. */
function newer(held, session) {
  if (!held) return true;
  if (earlier(session.last_activity_ms, held.last_activity_ms)) return false;
  if (!sameSession(held, session)) return true;
  return earlier(held.now_ms, session.now_ms) && standingOf(held) !== standingOf(session);
}

/** Write what one answer carried, if it is news and still belongs to its caller. */
export function writeUserSession(deviceId, answer, accept = () => true) {
  const session = userSessionOf(answer);
  if (!session) return Promise.resolve(false);
  return mergeCachedAtomically(userSessionAddress(deviceId), (held) =>
    (accept() && newer(held, session) ? session : null));
}

/** Write what one `issues.list` answer, or one page of it, says of the
 *  session. A bridge carrying Done since you left sends the session on every
 *  list and every page, so an answer with none is from one that does not — an
 *  older bridge, after a rollback — and the session a newer one left is
 *  dropped with it. The Dashboard reads a held session as the bridge carrying
 *  Done since you left (core/trackerIssuesPane.js); a stale one would cut
 *  Done off at a time rows without `done_at` cannot answer to (#104 review). */
export async function writeListedUserSession(deviceId, answer) {
  if (!answer) return false;
  if (userSessionOf(answer)) return writeUserSession(deviceId, answer);
  if (!(await readCached(userSessionAddress(deviceId)))) return false;
  await deleteCached([userSessionAddress(deviceId)]);
  return true;
}

export async function readUserSession(deviceId) {
  const record = await readCached(userSessionAddress(deviceId));
  return record?.value || null;
}
