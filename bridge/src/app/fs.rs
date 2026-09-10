use crate::app::{expand_tilde, mime_hint};

use std::io::Read as _;

use serde_json::{json, Value};

use super::{b64encode, fenced_scope_path, media_mime_hint, require_str, AppState, TermScope};

/// Source/document previews stay tightly capped; playable media gets a larger
/// bounded response because browsers cannot decode a truncated data URL.
pub(in crate::app) const FS_READ_MAX_BYTES: u64 = 1_048_576;

pub(in crate::app) const FS_MEDIA_READ_MAX_BYTES: u64 = 32 * 1_048_576;

impl AppState {
    /// Browse host directories so the user can pick a repo without typing a path.
    /// Returns the canonical path, its parent (for "up"), whether it is itself a git
    /// repo, and its subdirectories (each flagged if it is a git repo).
    pub(in crate::app) fn fs_list(&self, params: &Value) -> Result<Value, String> {
        let path = match params
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(p) => expand_tilde(p),
            None => expand_tilde("~"),
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let reader =
            std::fs::read_dir(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut dirs: Vec<(String, std::path::PathBuf, bool)> = reader
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| {
                let p = e.path();
                let is_git = p.join(".git").exists();
                (e.file_name().to_string_lossy().into_owned(), p, is_git)
            })
            .collect();
        dirs.sort_by_key(|a| a.0.to_lowercase());
        let entries: Vec<Value> = dirs
            .into_iter()
            .map(|(name, p, is_git)| {
                let is_hidden = name.starts_with('.');
                json!({ "name": name, "path": p.display().to_string(), "is_git": is_git, "is_hidden": is_hidden })
            })
            .collect();
        Ok(json!({
            "path": path.display().to_string(),
            "parent": path.parent().map(|p| p.display().to_string()),
            "is_git": path.join(".git").exists(),
            "entries": entries,
        }))
    }

    /// One directory level of a worktree-backed scope (spec §4.2): server-side
    /// scope resolution, the shared fence, `.git` skipped, dirs before
    /// files+symlinks, each group case-insensitive.
    pub(in crate::app) fn fs_tree(&mut self, params: &Value) -> Result<Value, String> {
        let scope = TermScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let target = fenced_scope_path(&root, &path)?;
        if !target.is_dir() {
            return Err("not a directory".to_string());
        }
        let reader = std::fs::read_dir(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let mut dirs: Vec<(String, Value)> = Vec::new();
        let mut rest: Vec<(String, Value)> = Vec::new();
        for entry in reader.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                dirs.push((name.clone(), json!({ "name": name, "kind": "dir" })));
            } else if file_type.is_symlink() {
                rest.push((name.clone(), json!({ "name": name, "kind": "symlink" })));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                rest.push((
                    name.clone(),
                    json!({ "name": name, "kind": "file", "size": size }),
                ));
            }
        }
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        rest.sort_by_key(|(name, _)| name.to_lowercase());
        let entries: Vec<Value> = dirs.into_iter().chain(rest).map(|(_, v)| v).collect();
        Ok(json!({ "path": path, "entries": entries }))
    }

    /// Read one file from a worktree-backed scope, base64 always, capped at the
    /// source limit or the larger bounded media limit server-side.
    pub(in crate::app) fn fs_read(&mut self, params: &Value) -> Result<Value, String> {
        let scope = TermScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = require_str(params, "path")?;
        let target = fenced_scope_path(&root, &path)?;
        let leaf =
            std::fs::symlink_metadata(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        if leaf.file_type().is_symlink() {
            return Err("refusing to read a symlink".to_string());
        }
        if leaf.is_dir() {
            return Err("not a file".to_string());
        }
        let size = leaf.len();
        let file = std::fs::File::open(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let read_limit = if media_mime_hint(&target).is_some() {
            FS_MEDIA_READ_MAX_BYTES
        } else {
            FS_READ_MAX_BYTES
        };
        let mut content = Vec::with_capacity(size.min(read_limit) as usize);
        file.take(read_limit)
            .read_to_end(&mut content)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        let truncated = size > read_limit;
        let head_len = content.len().min(8192);
        let mime = mime_hint(&target, &content[..head_len]);
        Ok(json!({
            "path": path,
            "size": size,
            "truncated": truncated,
            "mime": mime,
            "content_b64": b64encode(&content),
        }))
    }
}
