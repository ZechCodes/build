use crate::api::v1::git::{FsCreateDirectoryParams, FsCreateDirectoryResult, ScopeParams};
use crate::api::v1::WireParams;
use crate::api::ApiError;
use crate::app::{expand_tilde, mime_hint};
use crate::body_page::{page_len, BodyRange, BodySpan, FileRange};
use crate::encoding::b64decode;
use crate::scoped_file::{is_editable, replace_text, revision_hex, EDITABLE_MAX_BYTES};

use std::io::Read as _;

use serde_json::{json, Value};

use super::{b64encode, fenced_scope_path, media_mime_hint, require_str, AppState, TermScope};

/// The directory whose files an `fs.*` call may reach.
///
/// Workspace and project browsing name one configured source explicitly. The legacy
/// worktree-backed shapes remain valid for clients that have not learned
/// workspace sources yet; they continue to resolve through `TermScope`.
#[derive(Debug, Clone)]
enum FileScope {
    WorkspaceSource {
        workspace_id: String,
        source_id: String,
    },
    ProjectSource {
        project_id: String,
        source_id: String,
    },
    Legacy(TermScope),
}

impl FileScope {
    fn parse(params: &Value) -> Result<Self, String> {
        let workspace_id = scope_field(params, "workspace_id")?;
        let source_id = scope_field(params, "source_id")?;
        if workspace_id.is_none() && params.get("project_id").is_some() {
            if let Some(source_id) = source_id {
                return Self::project_source(params, source_id);
            }
        }
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

    fn project_source(params: &Value, source_id: String) -> Result<Self, String> {
        if ["run_id", "worktree_id"]
            .iter()
            .any(|name| params.get(*name).is_some_and(|value| !value.is_null()))
        {
            return Err(
                "project source scope cannot be combined with run or worktree scope ids".into(),
            );
        }
        let project_id = scope_field(params, "project_id")?
            .ok_or_else(|| "missing required param: project_id".to_string())?;
        Ok(Self::ProjectSource {
            project_id,
            source_id,
        })
    }

    fn resolve_root(&self, state: &mut AppState) -> Result<std::path::PathBuf, String> {
        match self {
            Self::WorkspaceSource {
                workspace_id,
                source_id,
            } => state.resolve_workspace_source(workspace_id, source_id),
            Self::ProjectSource {
                project_id,
                source_id,
            } => {
                let project = state
                    .projects
                    .get(project_id)
                    .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
                let source = project
                    .sources
                    .iter()
                    .find(|source| &source.id == source_id)
                    .ok_or_else(|| {
                        format!("unknown source_id {source_id} in project {project_id}")
                    })?;
                Ok(source.path.clone())
            }
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

/// Source/document previews stay tightly capped. Media is fetched in exact
/// 1 MiB pages for Blob playback, bounded to 64 MiB in the file viewer: this
/// covers the 50 MiB agent attachment ceiling while limiting IndexedDB and
/// phone memory. Each page's base64 JSON stays under the 8 MiB wire limit.
pub(in crate::app) const FS_READ_MAX_BYTES: u64 = 1_048_576;

pub(in crate::app) const FS_MEDIA_READ_MAX_BYTES: u64 = 64 * 1_048_576;

/// Keep the old unranged answer's behavior for clients that do not know raw
/// pages. A whole 64 MiB base64 answer would exceed DataChannel reassembly.
const FS_MEDIA_LEGACY_READ_MAX_BYTES: u64 = 32 * 1_048_576;

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
    pub(crate) fn fs_create_directory(
        &mut self,
        params: FsCreateDirectoryParams,
    ) -> Result<FsCreateDirectoryResult, ApiError> {
        let (scope, root) = self.file_mutation_scope(&params.scope)?;
        let destination =
            crate::scoped_upload::Destination::open(&root, &params.parent, &params.name)?;
        destination.create_directory()?;
        self.finish_file_mutation(&scope)?;
        Ok(FsCreateDirectoryResult {
            path: destination.path().into(),
        })
    }

    fn file_mutation_scope(
        &mut self,
        params: &ScopeParams,
    ) -> Result<(FileScope, std::path::PathBuf), ApiError> {
        let scope = FileScope::parse(&params.wire()).map_err(ApiError::classify)?;
        let root = scope.resolve_root(self).map_err(ApiError::classify)?;
        self.refuse_writers_while_reserved(&root)
            .map_err(ApiError::classify)?;
        Ok((scope, root))
    }

    fn finish_file_mutation(&mut self, scope: &FileScope) -> Result<(), ApiError> {
        if let FileScope::WorkspaceSource { workspace_id, .. } = scope {
            self.reopen_workspace(workspace_id)
                .map_err(ApiError::classify)?;
        }
        self.invalidate_file_scope(scope);
        Ok(())
    }

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

    /// One directory level of a source or worktree scope (spec §4.2): server-side
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

    /// Read one file from a source or worktree scope, base64 always, capped at the
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
        if let Some(range) = FileRange::from_params(params)? {
            return read_page(
                file,
                &target,
                &path,
                range.body(),
                range.raw.unwrap_or(false),
            );
        }
        let size = opened_metadata.len();
        let read_limit = if media_mime_hint(&target).is_some() {
            FS_MEDIA_LEGACY_READ_MAX_BYTES
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
            FileScope::WorkspaceSource { .. } | FileScope::ProjectSource { .. } => {}
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

/// One page of an open file (`fs.read` with a `range`, #95): whole lines for
/// text, exact bytes for raw media. A page is never editable, and a playable
/// file above the media cap returns truncated metadata without page bytes.
/// The mime is sniffed from the file's head, wherever the page starts.
fn read_page(
    mut file: std::fs::File,
    target: &std::path::Path,
    path: &str,
    range: BodyRange,
    raw: bool,
) -> Result<Value, String> {
    let ((head, window, offset), metadata) = read_unchanged(&mut file, path, |file, size| {
        cut_page(file, size, path, range, raw)
    })?;
    let size = metadata.len();
    let mime = mime_hint(target, &head);
    if raw {
        if !is_blob_media(mime) {
            return Err("raw range must name an image, audio, or video file".to_string());
        }
        if size > FS_MEDIA_READ_MAX_BYTES {
            return Ok(json!({
                "path": path,
                "size": size,
                "truncated": true,
                "mime": mime,
                "content_b64": "",
                "editable": false,
            }));
        }
    }
    let span = BodySpan {
        offset,
        end: offset + window.len() as u64,
        total: size,
        version: Some(file_version(&metadata)),
    };
    Ok(json!({
        "path": path,
        "size": size,
        "truncated": false,
        "mime": mime,
        "content_b64": b64encode(&window),
        "editable": false,
        "range": span,
    }))
}

/// How many times a page is read again when the file moves under the read.
const PAGE_READ_ATTEMPTS: usize = 3;

/// `read` run against `file` until the file's version is the same after it
/// as before, so the bytes it answers are the version they are named by: a
/// file rewritten in place between the two would otherwise hand a new page
/// under the old version, and a client would join it to the old pages. A
/// file that keeps moving is refused after a few tries — the client reads it
/// again from the top.
fn read_unchanged<T>(
    file: &mut std::fs::File,
    path: &str,
    mut read: impl FnMut(&mut std::fs::File, u64) -> Result<T, String>,
) -> Result<(T, std::fs::Metadata), String> {
    let stat = |file: &std::fs::File| {
        file.metadata()
            .map_err(|e| format!("cannot read {path}: {e}"))
    };
    for _ in 0..PAGE_READ_ATTEMPTS {
        let before = stat(file)?;
        let read = read(file, before.len())?;
        let after = stat(file)?;
        if file_version(&before) == file_version(&after) {
            return Ok((read, after));
        }
    }
    Err(format!("{path} kept changing while it was read"))
}

/// The head of a `size`-byte file, the page from `range.offset` and that
/// offset, clamped to the end.
fn cut_page(
    file: &mut std::fs::File,
    size: u64,
    path: &str,
    range: BodyRange,
    raw: bool,
) -> Result<(Vec<u8>, Vec<u8>, u64), String> {
    let offset = range.offset.min(size);
    let head = read_at(file, 0, 8192, path)?;
    if raw && size > FS_MEDIA_READ_MAX_BYTES {
        return Ok((head, Vec::new(), offset));
    }
    let mut window = read_at(file, offset, range.capacity() as u64, path)?;
    if !raw {
        let reaches_end = offset + window.len() as u64 >= size;
        window.truncate(page_len(&window, reaches_end));
    }
    Ok((head, window, offset))
}

fn is_blob_media(mime: &str) -> bool {
    (mime.starts_with("image/") && mime != "image/svg+xml")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
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
/// size, which move whenever its bytes do, and where the platform says them,
/// its change time and inode — a file renamed over this one, or one whose
/// modification time was set back, is another version too. As fine as the
/// filesystem's clock and no finer: a rewrite to the same size within one
/// tick of it keeps the version.
fn file_version(metadata: &std::fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        format!(
            "{modified}-{}.{}-{}-{}",
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.ino(),
            metadata.len()
        )
    }
    #[cfg(not(unix))]
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
mod page_tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    /// Rewrite `path` in place — same inode, same size — stamped `at`
    /// seconds, so the rewrite moves the version whatever the clock's tick.
    fn rewrite(path: &std::path::Path, byte: u8, at: u64) {
        std::fs::write(path, vec![byte; 8192]).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(at))
            .unwrap();
    }

    /// A file rewritten while a page is read is read again (#95 round 2): the
    /// page answered is the version it is named by, never new bytes under
    /// the version read before them.
    #[test]
    fn a_page_read_while_the_file_is_rewritten_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        rewrite(&path, b'A', 1_000);
        let mut file = std::fs::File::open(&path).unwrap();
        let range = BodyRange {
            offset: 4096,
            bytes: 4096,
        };
        let mut reads = 0;
        let ((_, window, offset), metadata) = read_unchanged(&mut file, "big.log", |file, size| {
            let page = cut_page(file, size, "big.log", range, false);
            reads += 1;
            if reads == 1 {
                rewrite(&path, b'B', 2_000);
            }
            page
        })
        .unwrap();
        assert_eq!(reads, 2);
        assert_eq!((offset, window), (4096, vec![b'B'; 4096]));
        let now = std::fs::metadata(&path).unwrap();
        assert_eq!(file_version(&metadata), file_version(&now));
    }

    #[test]
    fn a_file_that_keeps_changing_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        rewrite(&path, b'A', 1_000);
        let mut file = std::fs::File::open(&path).unwrap();
        let mut stamp = 1_000;
        let error = read_unchanged(&mut file, "big.log", |_, _| {
            stamp += 1;
            rewrite(&path, b'A', stamp);
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("kept changing"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_renamed_over_the_old_one_is_another_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        let beside = dir.path().join("big.log.new");
        rewrite(&path, b'A', 1_000);
        rewrite(&beside, b'B', 1_000);
        let before = std::fs::metadata(&path).unwrap();
        std::fs::rename(&beside, &path).unwrap();
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_ne!(file_version(&before), file_version(&after));
    }
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
