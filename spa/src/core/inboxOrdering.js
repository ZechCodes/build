// Shared ordering policy for Inbox and Projects, apart from row markup.

export const RECENT_AFTER_MS = 24 * 3600 * 1000;

/** Running work is active now. A manual clear keeps its existing override;
 * unknown activity is not evidence that a row is old. */
export const workspaceIsRecent = (entry, nowMs = Date.now()) =>
  !!entry.dismissed || (!entry.working && Number.isFinite(entry.lastActivityMs)
    && nowMs - entry.lastActivityMs >= RECENT_AFTER_MS);

/** The earliest start of a current activity streak goes first. Unknown
 * starts follow dated entries, retaining their stable input order. */
export function byAnchor(left, right) {
  if ((left.anchorMs ?? null) === (right.anchorMs ?? null)) return 0;
  if (left.anchorMs == null) return 1;
  if (right.anchorMs == null) return -1;
  return left.anchorMs - right.anchorMs;
}

/** Recent rows cannot hold a project above one with active work. The project
 * conversation participates alongside its watched workspaces and tasks. */
export function longestActiveAnchor(entries, nowMs) {
  const active = entries.filter((entry) => !workspaceIsRecent(entry, nowMs)
    && Number.isFinite(entry.anchorMs));
  return active.reduce((oldest, entry) => Math.min(oldest ?? entry.anchorMs, entry.anchorMs), null);
}
