use crate::app::{expand_tilde, mime_hint};
use crate::body_page::{page_len, BodyRange, BodySpan};
use crate::encoding::b64decode;
use crate::scoped_file::{is_editable, replace_text, revision_hex, EDITABLE_MAX_BYTES};

use std::io::Read as _;

use serde_json::{json, Value};

use super::{b64encode, fenced_scope_path, media_mime_hint, require_str, AppState, TermScope};

/// The directory whose files an `fs.*` call may reach.
///
/// Workspace browsing names one configured source explicitly. The legacy
/// worktree-backed shapes remain valid for clients that have not learned
/// workspace sources yet; they continue to resolve through `TermScope`.
#[derive(Debug, Clone)]
enum FileScope {
    WorkspaceSource {
        workspace_id: String,
        source_id: String,
    },
    Legacy(TermScope),
}

impl FileScope {
    fn parse(params: &Value) -> Result<Self, String> {
        let workspace_id = scope_field(params, "workspace_id")?;
        let source_id = scope_field(params, "source_id")?;
        let has_legacy_scope = ["run_id", "project_id", "worktree_id"]
            .iter()
            .any(|name| params.get(*name).is_some_and(|value| !value.is_null()));
        if (workspace_id.is_some() || source_id.is_some()) && has_legacy_scope {
            return Err(
                "workspace source scope cannot be combined with legacy scope ids".to_string(),
            );
        }
        match (workspace_id, source_id) {
            (Some(workspace_id), Some(source_id)) => Ok(Self::WorkspaceSource {
                workspace_id,
                source_id,
            }),
            (Some(_), None) => Err("missing required param: source_id".to_string()),
            (None, Some(_)) => Err("missing required param: workspace_id".to_string()),
            (None, None) => TermScope::parse(params).map(Self::Legacy),
        }
    }

    fn resolve_root(&self, state: &mut AppState) -> Result<std::path::PathBuf, String> {
        match self {
            Self::WorkspaceSource {
                workspace_id,
                source_id,
            } => state.resolve_workspace_source(workspace_id, source_id),
            Self::Legacy(scope) => scope.resolve_root(state),
        }
    }
}

fn scope_field(params: &Value, name: &str) -> Result<Option<String>, String> {
    match params.get(name) {
        None => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Err(format!("{name} cannot be empty")),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{name} must be a string")),
    }
}

/// Source/document previews stay tightly capped; playable media gets a larger
/// bounded response because browsers cannot decode a truncated data URL.
pub(in crate::app) const FS_READ_MAX_BYTES: u64 = 1_048_576;

pub(in crate::app) const FS_MEDIA_READ_MAX_BYTES: u64 = 32 * 1_048_576;

/// One directory level of a resolved scope: the shared fence, `.git` skipped,
/// dirs before files and symlinks, each group case-insensitive.
///
/// The body of `fs.tree`, and what a `files` push item carries for the root —
/// one listing, so a client painting from a push and a client painting from a
/// read are looking at the same thing.
pub(in crate::app) fn directory_listing(
    root: &std::path::Path,
    path: &str,
) -> Result<Value, String> {
    let target = fenced_scope_path(root, path)?;
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

impl AppState {
    /// Browse host directories so the user can pick a repo without typing a path.
    /// Returns the canonical path, its parent (for "up"), whether it is itself a git
    /// repo, and its subdirectories (each flagged if it is a git repo).
    pub(crate) fn fs_list(&self, params: &Value) -> Result<Value, String> {
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

    /// Create one plain directory in a host folder selected by the user.
    /// The parent follows `fs.list`'s host-browsing authority; accepting the
    /// child separately keeps traversal and implicit parent creation out of the
    /// mutation surface.
    pub(crate) fn fs_mkdir(&self, params: &Value) -> Result<Value, String> {
        let parent_value = require_str(params, "parent")?;
        let parent = std::fs::canonicalize(expand_tilde(&parent_value))
            .map_err(|error| format!("cannot open parent folder: {error}"))?;
        if !parent.is_dir() {
            return Err("parent is not a directory".to_string());
        }
        let name = require_str(params, "name")?;
        let is_single_component = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && std::path::Path::new(&name)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)));
        if !is_single_component {
            return Err("name must be a single folder name".to_string());
        }
        self.refuse_writers_while_reserved(&parent)?;
        let target = parent.join(name);
        std::fs::create_dir(&target)
            .map_err(|error| format!("cannot create {}: {error}", target.display()))?;
        let path = std::fs::canonicalize(&target)
            .map_err(|error| format!("cannot open created folder: {error}"))?;
        Ok(json!({ "path": path.display().to_string() }))
    }

    /// One directory level of a worktree-backed scope (spec §4.2): server-side
    /// scope resolution, the shared fence, `.git` skipped, dirs before
    /// files+symlinks, each group case-insensitive.
    pub(crate) fn fs_tree(&mut self, params: &Value) -> Result<Value, String> {
        let scope = FileScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        directory_listing(&root, &path)
    }

    /// Read one file from a worktree-backed scope, base64 always, capped at the
    /// source limit or the larger bounded media limit server-side.
    pub(crate) fn fs_read(&mut self, params: &Value) -> Result<Value, String> {
        let scope = FileScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = require_str(params, "path")?;
        let target = fenced_scope_path(&root, &path)?;
        let leaf =
            std::fs::symlink_metadata(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        if leaf.file_type().is_symlink() {
            return Err("refusing to read a symlink".to_string());
        }
        let (file, opened_metadata) = open_regular_read(&target, &path)?;
        if let Some(range) = BodyRange::from_params(params)? {
            return read_page(file, &opened_metadata, &target, &path, range);
        }
        let size = opened_metadata.len();
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
        let editable = is_editable(&content, !truncated) && text_mime(mime);
        let revision = (!truncated).then(|| revision_hex(&content));
        Ok(json!({
            "path": path,
            "size": size,
            "truncated": truncated,
            "mime": mime,
            "content_b64": b64encode(&content),
            "editable": editable,
            "encoding": editable.then_some("utf-8"),
            "revision": revision,
        }))
    }

    /// Replace an existing editable text file after verifying the exact bytes
    /// the editor opened. The descriptor-relative helper holds the fenced
    /// parent through the atomic rename, defeating ancestor symlink swaps.
    pub(crate) fn fs_write(&mut self, params: &Value) -> Result<Value, String> {
        let scope = FileScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = require_str(params, "path")?;
        let expected_revision = require_str(params, "expected_revision")?;
        let encoded = require_str(params, "content_b64")?;
        if encoded.len() > (EDITABLE_MAX_BYTES * 4).div_ceil(3) + 4 {
            return Err("replacement exceeds the 1048576-byte limit".to_string());
        }
        let replacement = b64decode(&encoded)?;
        if replacement.len() > EDITABLE_MAX_BYTES {
            return Err("replacement exceeds the 1048576-byte limit".to_string());
        }

        self.refuse_writers_while_reserved(&root)?;
        let current = self.fs_read(params)?;
        if current.get("editable") != Some(&Value::Bool(true)) {
            return Err("file is not editable UTF-8 text".to_string());
        }
        replace_text(&root, &path, &expected_revision, &replacement)?;
        if let FileScope::WorkspaceSource { workspace_id, .. } = &scope {
            self.reopen_workspace(workspace_id)?;
        }
        self.invalidate_file_scope(&scope);
        self.fs_read(params)
    }

    fn invalidate_file_scope(&mut self, scope: &FileScope) {
        match scope {
            FileScope::WorkspaceSource { .. } => {}
            FileScope::Legacy(TermScope::Run { run_id }) => {
                self.invalidate_run_stat(run_id);
                self.note_entity_changed(run_id);
            }
            FileScope::Legacy(TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            }) => {
                self.rescan_external_worktrees(project_id);
                self.note_entity_changed(worktree_id);
            }
            // The project's own checkout carries no board summary of its
            // own: the board lists workspaces.
            FileScope::Legacy(TermScope::Primary { .. }) => {}
        }
        self.note_board_changed();
    }
}

/// One page of an open file (`fs.read` with a `range`, #95): the bytes from
/// the offset, cut after the last line end that fits. Never editable — a page
/// is not the file — and never truncated: it is what was asked for. The mime
/// is sniffed from the file's head, wherever the page starts, so every page
/// of one file names the same type.
fn read_page(
    mut file: std::fs::File,
    metadata: &std::fs::Metadata,
    target: &std::path::Path,
    path: &str,
    range: BodyRange,
) -> Result<Value, String> {
    use std::io::{Seek as _, SeekFrom};
    let size = metadata.len();
    let offset = range.offset.min(size);
    let head = read_at(&mut file, 0, 8192, path)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut window = Vec::with_capacity(range.capacity().min((size - offset) as usize));
    (&mut file)
        .take(range.capacity() as u64)
        .read_to_end(&mut window)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let reaches_end = offset + window.len() as u64 >= size;
    window.truncate(page_len(&window, reaches_end));
    let span = BodySpan {
        offset,
        end: offset + window.len() as u64,
        total: size,
        version: Some(file_version(metadata)),
    };
    Ok(json!({
        "path": path,
        "size": size,
        "truncated": false,
        "mime": mime_hint(target, &head),
        "content_b64": b64encode(&window),
        "editable": false,
        "range": span,
    }))
}

/// Up to `bytes` of `file` from `offset`.
fn read_at(
    file: &mut std::fs::File,
    offset: u64,
    bytes: u64,
    path: &str,
) -> Result<Vec<u8>, String> {
    use std::io::{Seek as _, SeekFrom};
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut read = Vec::new();
    file.take(bytes)
        .read_to_end(&mut read)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    Ok(read)
}

/// Which version of a file a page was cut from: its modification time and
/// size, which move whenever its bytes do.
fn file_version(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    format!("{modified}-{}", metadata.len())
}

fn text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || matches!(mime, "application/json" | "image/svg+xml")
}

#[cfg(unix)]
fn open_regular_read(
    path: &std::path::Path,
    display_path: &str,
) -> Result<(std::fs::File, std::fs::Metadata), String> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("cannot read {display_path}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {display_path}: {error}"))?;
    if !metadata.is_file() {
        return Err("not a file".to_string());
    }
    Ok((file, metadata))
}

#[cfg(not(unix))]
fn open_regular_read(
    path: &std::path::Path,
    display_path: &str,
) -> Result<(std::fs::File, std::fs::Metadata), String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("cannot read {display_path}: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {display_path}: {error}"))?;
    if !metadata.is_file() {
        return Err("not a file".to_string());
    }
    Ok((file, metadata))
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn workspace_file_scope_requires_both_ids() {
        for (params, missing) in [
            (json!({ "workspace_id": "workspace-1" }), "source_id"),
            (json!({ "source_id": "source-1" }), "workspace_id"),
        ] {
            let error = FileScope::parse(&params).unwrap_err();
            assert_eq!(error, format!("missing required param: {missing}"));
        }
    }

    #[test]
    fn workspace_file_scope_does_not_fall_through_to_a_legacy_scope() {
        let error = FileScope::parse(&json!({
            "workspace_id": "workspace-1",
            "source_id": "source-1",
            "project_id": "proj-1",
        }))
        .unwrap_err();
        assert_eq!(
            error,
            "workspace source scope cannot be combined with legacy scope ids"
        );
    }

    #[test]
    fn legacy_file_scopes_remain_valid() {
        assert!(matches!(
            FileScope::parse(&json!({ "project_id": "proj-1" })).unwrap(),
            FileScope::Legacy(TermScope::Primary { project_id }) if project_id == "proj-1"
        ));
        assert!(matches!(
            FileScope::parse(&json!({ "run_id": "run-1" })).unwrap(),
            FileScope::Legacy(TermScope::Run { run_id }) if run_id == "run-1"
        ));
    }

    #[test]
    fn workspace_tree_is_fenced_to_the_selected_source() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = AppState::new_unrooted(
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let sources = [
            crate::workspace::WorkspaceSource {
                id: "source-1".to_string(),
                name: "frontend".to_string(),
                mount: "frontend".to_string(),
                path: directory.path().join("frontend-source"),
                is_git: false,
                base_branch: "main".to_string(),
            },
            crate::workspace::WorkspaceSource {
                id: "source-2".to_string(),
                name: "api".to_string(),
                mount: "api".to_string(),
                path: directory.path().join("api-source"),
                is_git: false,
                base_branch: "main".to_string(),
            },
        ];
        let workspace = state
            .workspaces
            .begin("proj-1", "selected-source", &sources)
            .unwrap();
        for source in &workspace.directories {
            std::fs::create_dir_all(&source.path).unwrap();
            std::fs::write(
                source.path.join(format!("{}.txt", source.name)),
                &source.name,
            )
            .unwrap();
        }

        let tree = state
            .fs_tree(&json!({
                "workspace_id": workspace.id,
                "source_id": "source-2",
            }))
            .unwrap();
        assert_eq!(tree["entries"].as_array().unwrap().len(), 1);
        assert_eq!(tree["entries"][0]["name"], "api.txt");

        let escape = state.fs_tree(&json!({
            "workspace_id": workspace.id,
            "source_id": "source-2",
            "path": "../frontend",
        }));
        assert_eq!(escape.unwrap_err(), "path escapes the worktree");
    }
}
