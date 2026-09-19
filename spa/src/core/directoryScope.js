// A workspace's repositories may contain identical paths and commit IDs.
// Keep directory-local caches and drafts separate while sharing workspace events.
export function directoryCacheId(scope) {
  if (scope?.workspace_id) return `workspace:${JSON.stringify([scope.workspace_id, scope.source_id])}`;
  return scope?.run_id || scope?.worktree_id || null;
}

/**
 * Whether the sync layer walks this checkout — which decides whether a record
 * filed under it is the answer or only a seed.
 *
 * The pass walks the board's rows, and a row's git scope is a run or an
 * external worktree (core/cacheSync.js `gitScopeOf`). Two checkouts are on no
 * such row: a workspace SOURCE, whose records are filed under the source and
 * only the surface standing on it ever names one, and a project's own
 * directory, which is no entity at all. Nothing re-lists a tree or re-reads a
 * body for those, so a surface over one reads for itself.
 */
export function syncWalksCheckout(scope) {
  if (!scope || scope.workspace_id) return false;
  return Boolean(scope.run_id || scope.worktree_id);
}
