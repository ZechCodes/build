// A workspace's repositories may contain identical paths and commit IDs.
// Keep directory-local caches and drafts separate while sharing workspace events.
export function directoryCacheId(scope) {
  if (scope?.workspace_id) return `workspace:${JSON.stringify([scope.workspace_id, scope.source_id])}`;
  return scope?.run_id || scope?.worktree_id || null;
}
