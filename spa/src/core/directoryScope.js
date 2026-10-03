// A workspace's repositories may contain identical paths and commit IDs.
// Keep directory-local caches and drafts separate while sharing workspace events.
export function directoryCacheId(scope) {
  if (!scope) return null;
  // The workspace's own git directory is what its conversation's run stands on
  // for git: the bridge answers run-scoped reads from it and pushes its facts
  // under the run. A scope that names that entity files its records there —
  // where the sync layer writes and the pushes land — so it carries `entity_id`.
  const ownedEntity = [scope.entity_id, scope.run_id, scope.worktree_id].find(Boolean);
  if (ownedEntity) return ownedEntity;
  if (scope.workspace_id) return `workspace:${JSON.stringify([scope.workspace_id, scope.source_id])}`;
  if (scope.project_id) return `project:${JSON.stringify(scope.source_id ? [scope.project_id, scope.source_id] : [scope.project_id])}`;
  return null;
}

/** The cache entity a workspace's own Files state is filed under — the open
 *  tabs and the folded roots that stand across its directories (#174). Apart
 *  from every directory's own (`workspace:[id, source]`), and from any run's. */
export const workspaceLayoutCacheId = (workspaceId) => `workspace:${JSON.stringify([workspaceId])}`;

/**
 * Whether the sync layer walks this checkout — which decides whether a record
 * filed under it is the answer or only a seed.
 *
 * The pass walks the board's rows, and a row's git scope is a run or an
 * external worktree (core/cacheSync.js `gitScopeOf`). Two checkouts are on no
 * such row: a workspace SOURCE, whose records are filed under the source and
 * only the surface standing on it ever names one, and a project's own
 * directory, whose synthetic cache entity is not a board row. Nothing re-lists
 * a tree or re-reads a body for those, so a surface over one reads for itself.
 */
export function syncWalksCheckout(scope) {
  if (!scope) return false;
  if (scope.entity_id) return true;
  if (scope.workspace_id) return false;
  return Boolean(scope.run_id || scope.worktree_id);
}

/** Project-wide tabs and folded roots stay apart from each source's records. */
export const projectLayoutCacheId = (projectId) => `project-files:${JSON.stringify([projectId])}`;
