// The hand-off between naming a worktree and landing in it.
//
// worktree.create gives back a directory with nothing running in it, and the
// sheet already asked what should be — so the create leaves a one-shot marker
// carrying that answer, keyed by worktree id, and the surface consumes it to
// open that tab on arrival instead of a cold Changes view of an empty diff.
//
// A marker rather than a route param: it must survive the navigation, fire
// exactly once, and never re-fire on a reload of that surface.

const MARKER_PREFIX = "build.newWorktree.";

/** Mark a freshly created worktree with the tool to open in it. */
export function markNewWorktree(worktreeId, kind, storage = sessionStorage) {
  if (!worktreeId || !kind) return;
  try {
    storage.setItem(MARKER_PREFIX + worktreeId, String(kind));
  } catch {
    /* private mode: the surface just opens on its default tab */
  }
}

/** Consume the marker, returning the tool to open — at most once per worktree,
 *  and null when there is nothing waiting. */
export function takeNewWorktreeMark(worktreeId, storage = sessionStorage) {
  if (!worktreeId) return null;
  try {
    const key = MARKER_PREFIX + worktreeId;
    const kind = storage.getItem(key);
    if (kind) storage.removeItem(key);
    return kind || null;
  } catch {
    return null;
  }
}
