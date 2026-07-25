// Minting a bare worktree from the FAB, and the hand-off that makes the new
// surface useful the moment it opens.
//
// worktree.create gives back a directory with nothing in it — no run, no
// session, no agent. What you want next is always the same thing: something
// running in it. So the create leaves a one-shot marker keyed by worktree id,
// and the worktree surface consumes it to open a chooser tab (Claude Code /
// Codex / Terminal) instead of a cold Changes view of an empty diff.
//
// A marker rather than a route param: it must survive the navigation, fire
// exactly once, and never re-fire on a reload of that surface.

const MARKER_PREFIX = "build.newWorktree.";

/** Mark a freshly created worktree as wanting the chooser tab. */
export function markNewWorktree(worktreeId, storage = sessionStorage) {
  if (!worktreeId) return;
  try {
    storage.setItem(MARKER_PREFIX + worktreeId, "1");
  } catch {
    /* private mode: the surface just opens on its default tab */
  }
}

/** Consume the marker — true at most once per created worktree. */
export function takeNewWorktreeMark(worktreeId, storage = sessionStorage) {
  if (!worktreeId) return false;
  try {
    const key = MARKER_PREFIX + worktreeId;
    const marked = storage.getItem(key) === "1";
    if (marked) storage.removeItem(key);
    return marked;
  } catch {
    return false;
  }
}

/**
 * Create a worktree in `projectId` and route to it with the chooser armed.
 * `callRpc` is the app RPC, `navigate` the router's go(). Rejects with the
 * bridge's error so the caller can surface it; the FAB re-arms on rejection.
 */
export async function createWorktreeAndOpen({ projectId, callRpc, navigate, storage = sessionStorage }) {
  const created = await callRpc("worktree.create", { project_id: projectId });
  markNewWorktree(created.worktree_id, storage);
  navigate({ name: "worktree", projectId: created.project_id || projectId, worktreeId: created.worktree_id });
  return created;
}
