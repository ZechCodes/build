// The hand-off between naming a worktree and landing in it.
//
// worktree.create gives back a directory with nothing running in it, and the
// sheet already asked which agent works there — so the create leaves a one-shot
// marker carrying that provider, keyed by worktree id. The surface consumes it
// to open on the Agent tab (where that agent will appear) instead of a cold
// Changes view of an empty diff, and to dispatch with it when the first turn
// adopts the worktree.
//
// A marker rather than a route param: it must survive the navigation, fire
// exactly once, and never re-fire on a reload of that surface.

const MARKER_PREFIX = "build.newWorktree.";

/** Mark a freshly created worktree with the agent provider chosen for it. */
export function markNewWorktree(worktreeId, provider, storage = sessionStorage) {
  if (!worktreeId || !provider) return;
  try {
    storage.setItem(MARKER_PREFIX + worktreeId, String(provider));
  } catch {
    /* private mode: the surface just opens on its default tab */
  }
}

/** Consume the marker, returning the chosen provider — at most once per
 *  worktree, and null when there is nothing waiting. */
export function takeNewWorktreeMark(worktreeId, storage = sessionStorage) {
  if (!worktreeId) return null;
  try {
    const key = MARKER_PREFIX + worktreeId;
    const provider = storage.getItem(key);
    if (provider) storage.removeItem(key);
    return provider || null;
  } catch {
    return null;
  }
}
