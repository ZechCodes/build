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
