//! Canonical containment checks for paths inside a filesystem scope.

use std::path::{Component, Path, PathBuf};

/// True iff `path` stays inside whatever directory it is joined under: it is
/// non-empty, relative, and made only of normal components — no `..`, no `.`
/// segments, no root. This is the fence that keeps agent-supplied paths (the
/// manifest echo, `plan_path`) from escaping the worktree or the store's docs
/// dir; a naive prefix check alone would accept
/// `.build/plan/../../../etc/passwd`.
pub fn is_worktree_contained_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Resolve a client-supplied relative `path` under a worktree-backed `root`,
/// fenced on both ends (spec §4.1): the lexical fence
/// (`is_worktree_contained_path` — no `..`, no root, no non-Normal component)
/// PLUS canonical containment, which is what actually defeats a symlink
/// pointing outside the root (a symlink's own path components are all
/// Normal, so the lexical fence alone cannot catch it). `path` empty means
/// the scope root itself. Returns the joined (not canonicalized) path — safe
/// to use for further fs calls once containment is established.
pub(crate) fn fenced_scope_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    if !path.is_empty() && !is_worktree_contained_path(path) {
        return Err("path escapes the worktree".to_string());
    }
    let joined = if path.is_empty() {
        root.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical_root =
        std::fs::canonicalize(root).map_err(|e| format!("cannot resolve scope root: {e}"))?;
    let canonical_target =
        std::fs::canonicalize(&joined).map_err(|e| format!("cannot read {path}: {e}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("path escapes the worktree".to_string());
    }
    Ok(joined)
}
