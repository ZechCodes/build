// Shared ordering policy for Inbox and Projects, apart from row markup.

export const RECENT_AFTER_MS = 24 * 3600 * 1000;

/** Running work is active now. A manual clear keeps its existing override;
 * unknown activity is not evidence that a row is old. The mounted editor may
 * hold its existing active partition without changing activity timestamps. */
export const workspaceIsRecent = (entry, nowMs = Date.now(), heldActiveKey = null) =>
  (heldActiveKey === null || entry.key !== heldActiveKey) && (!!entry.dismissed || (!entry.working && Number.isFinite(entry.lastActivityMs)
    && nowMs - entry.lastActivityMs >= RECENT_AFTER_MS));

/** The earliest start of a current activity streak goes first. Unknown
 * starts follow dated entries, retaining their stable input order. */
export function byAnchor(left, right) {
  if ((left.anchorMs ?? null) === (right.anchorMs ?? null)) return 0;
  if (left.anchorMs == null) return 1;
  if (right.anchorMs == null) return -1;
  return left.anchorMs - right.anchorMs;
}

/** The oldest dated streak among a project's standing rows. */
export function longestActiveAnchor(entries) {
  return entries.reduce((oldest, { anchorMs }) =>
    Number.isFinite(anchorMs) ? Math.min(oldest ?? anchorMs, anchorMs) : oldest, null);
}
