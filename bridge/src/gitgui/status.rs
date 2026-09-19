use super::history::{current_branch, head_commit_id, open_repo, GIT_STATUS_MAX_FILES};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

fn has_index_change(status: git2::Status) -> bool {
    status.intersects(
        git2::Status::INDEX_NEW
            | git2::Status::INDEX_MODIFIED
            | git2::Status::INDEX_DELETED
            | git2::Status::INDEX_RENAMED
            | git2::Status::INDEX_TYPECHANGE,
    )
}

/// An untracked path: present in the working tree, absent from the index
/// (never `git add`ed). This is the set `git.discard` deletes outright rather
/// than restoring from HEAD.
pub(super) fn is_untracked_status(status: git2::Status) -> bool {
    status.contains(git2::Status::WT_NEW) && !has_index_change(status)
}

/// Per-file staging tri-state + index/worktree letters for one statuses entry.
/// `None` for entries that carry no reportable change (e.g. ignored).
pub(super) fn file_status_json(path: &str, status: git2::Status) -> Option<Value> {
    if status.is_ignored() {
        return None;
    }
    // Unmerged entries carry CONFLICTED alone (no INDEX_*/WT_* bits), so they
    // must be classified before the changed-bits check or they vanish from
    // the staging surface entirely.
    if status.is_conflicted() {
        return Some(json!({
            "path": path,
            "staged": "none",
            "index_status": "U",
            "worktree_status": "U",
        }));
    }
    let index_changed = has_index_change(status);
    let worktree_changed = status.intersects(
        git2::Status::WT_NEW
            | git2::Status::WT_MODIFIED
            | git2::Status::WT_DELETED
            | git2::Status::WT_TYPECHANGE
            | git2::Status::WT_RENAMED,
    );
    if !index_changed && !worktree_changed {
        return None;
    }
    let untracked = status.contains(git2::Status::WT_NEW) && !index_changed;
    let (staged, index_status, worktree_status) = if untracked {
        ("none", "?", "?")
    } else {
        let index_status = if status.contains(git2::Status::INDEX_NEW) {
            "A"
        } else if status.contains(git2::Status::INDEX_DELETED) {
            "D"
        } else if index_changed {
            "M"
        } else {
            "-"
        };
        let worktree_status = if status.contains(git2::Status::WT_DELETED) {
            "D"
        } else if status.contains(git2::Status::WT_NEW) {
            "A"
        } else if worktree_changed {
            "M"
        } else {
            "-"
        };
        let staged = match (index_changed, worktree_changed) {
            (true, true) => "partial",
            (true, false) => "full",
            (false, _) => "none",
        };
        (staged, index_status, worktree_status)
    };
    Some(json!({
        "path": path,
        "staged": staged,
        "index_status": index_status,
        "worktree_status": worktree_status,
    }))
}

/// The full `git.status` payload for a checkout — also the response body of
/// `git.stage`/`git.unstage` and the `status` field of `git.commit`, so the
/// UI repaints straight from the mutation's response.
///
/// Shape and counts, never a patch: a file's body is fetched per path through
/// [`file_patches`] and cached against its `content_key`.
pub fn status_payload(repo_path: &Path) -> Result<Value, String> {
    status_payload_unless(repo_path, None)
}

/// [`status_payload`], unless the client already holds it.
///
/// `if_status_key` is the [`status_key`] the browser is painting. When it still
/// names the working tree, the answer is that fact alone — the status walk ran,
/// nothing else did, and no patch, count or body crossed the wire.
pub fn status_payload_unless(
    repo_path: &Path,
    if_status_key: Option<&str>,
) -> Result<Value, String> {
    let (shape, key) = status_shape(repo_path, GIT_STATUS_MAX_FILES)?;
    match if_status_key {
        Some(held) if held == key => Ok(json!({ "unchanged": true, "status_key": key })),
        _ => counted_status(shape, key, repo_path),
    }
}

/// Map a raw [`git2::RepositoryState`] to the wire vocabulary the SPA banner
/// keys on, before the working-tree conflict check: any rebase flavor is
/// "rebasing", a merge is "merging", a cherry-pick / revert / bisect maps to
/// its own label, a mailbox-apply (and anything future) is "other", and an
/// idle repo is "clean".
pub(super) fn map_repository_state(state: git2::RepositoryState) -> &'static str {
    match state {
        git2::RepositoryState::Clean => "clean",
        git2::RepositoryState::Merge => "merging",
        git2::RepositoryState::Rebase
        | git2::RepositoryState::RebaseInteractive
        | git2::RepositoryState::RebaseMerge => "rebasing",
        git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence => {
            "cherry-picking"
        }
        git2::RepositoryState::Revert | git2::RepositoryState::RevertSequence => "reverting",
        git2::RepositoryState::Bisect => "bisecting",
        _ => "other",
    }
}

/// Whether the index carries any unmerged (CONFLICTED) entry — the signal that
/// a repo whose [`git2::RepositoryState`] reports Clean is nonetheless
/// mid-conflict. The canonical case is a `git stash pop` that conflicts: it
/// leaves UU entries but no MERGE_HEAD, so `repo.state()` stays Clean.
fn has_conflicted_entry(repo: &git2::Repository) -> Result<bool, String> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    Ok(statuses.iter().any(|entry| entry.status().is_conflicted()))
}

/// The repo's operational state on the wire. It is the raw
/// [`git2::RepositoryState`] mapping, except a repo that maps to "clean" but
/// has any unmerged index entry reports "conflicted". The conflict check runs
/// only after the mapping resolves to "clean", so a merging/rebasing repo keeps
/// its own label rather than being flattened to "conflicted".
pub(super) fn repo_state_label(repo: &git2::Repository) -> Result<&'static str, String> {
    let label = map_repository_state(repo.state());
    if label == "clean" && has_conflicted_entry(repo)? {
        return Ok("conflicted");
    }
    Ok(label)
}

/// The current branch's upstream (as `origin/main`-style shorthand) plus its
/// ahead/behind counts against that upstream. All three are `None` when HEAD
/// is unborn, detached, or has no configured upstream — the SPA renders the
/// sync chips only when they are present.
pub(super) fn upstream_status(
    repo: &git2::Repository,
) -> (Option<String>, Option<u64>, Option<u64>) {
    let head = match repo.head() {
        Ok(head) => head,
        Err(_) => return (None, None, None),
    };
    if !head.is_branch() {
        return (None, None, None);
    }
    let local_oid = match head.target() {
        Some(oid) => oid,
        None => return (None, None, None),
    };
    let local = git2::Branch::wrap(head);
    let upstream = match local.upstream() {
        Ok(upstream) => upstream,
        Err(_) => return (None, None, None),
    };
    let name = upstream.name().ok().flatten().map(str::to_string);
    let upstream_oid = match upstream.get().target() {
        Some(oid) => oid,
        None => return (name, None, None),
    };
    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((ahead, behind)) => (name, Some(ahead as u64), Some(behind as u64)),
        Err(_) => (name, None, None),
    }
}

/// The number of entries on the stash stack. Takes `&mut` because
/// `stash_foreach` mutates the repository's stash iterator state.
fn count_stashes(repo: &mut git2::Repository) -> Result<u64, String> {
    let mut count = 0u64;
    repo.stash_foreach(|_, _, _| {
        count += 1;
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(count)
}

/// A stable name for a file's current WORKING-TREE content, which is what a
/// cached body must be keyed by. The index is not usable here: a tracked file
/// that is staged and then edited again keeps its index entry, so two
/// successive edits would share one key and a stale body would read as current.
pub(super) struct ContentKeys {
    worktree_root: std::path::PathBuf,
}

impl ContentKeys {
    /// The keys of one checkout's files.
    pub(super) fn of(repo: &git2::Repository) -> Self {
        ContentKeys {
            worktree_root: repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf(),
        }
    }

    /// libgit2's own working-tree object id when the status walk computed one,
    /// and the file's size and modification time hashed when it did not.
    pub(super) fn key_for(&self, path: &str, entry: &git2::StatusEntry<'_>) -> String {
        entry
            .index_to_workdir()
            .map(|delta| delta.new_file().id())
            .filter(|oid| !oid.is_zero())
            .map_or_else(|| self.key_off_disk(path), |oid| oid.to_string())
    }

    /// A path with nothing on disk is `deleted` — the one key that names an
    /// absence rather than a content.
    pub(super) fn key_off_disk(&self, path: &str) -> String {
        let Ok(metadata) = std::fs::symlink_metadata(self.worktree_root.join(path)) else {
            return "deleted".to_string();
        };
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        crate::diff::fnv1a64_hex(&format!("{}:{modified_nanos}", metadata.len()))
    }
}

/// The repository-wide fields a repaint depends on.
const STATUS_KEY_FIELDS: [&str; 8] = [
    "branch",
    "head",
    "repo_state",
    "upstream",
    "ahead",
    "behind",
    "stash_count",
    "files_truncated",
];

/// The per-file fields a repaint depends on. Line counts are not among them:
/// they follow the content key, which already moved.
const FILE_KEY_FIELDS: [&str; 6] = [
    "path",
    "staged",
    "index_status",
    "worktree_status",
    "content_key",
    "edited_at",
];

/// A stable 16-hex name for everything a client's repaint depends on, so a
/// poll that finds the same key can stop there.
fn status_key(shape: &Value) -> String {
    let mut material = String::new();
    let mut push = |value: &Value| {
        material.push_str(&value.to_string());
        material.push('|');
    };
    for field in STATUS_KEY_FIELDS {
        push(&shape[field]);
    }
    for file in shape["files"].as_array().into_iter().flatten() {
        for field in FILE_KEY_FIELDS {
            push(&file[field]);
        }
    }
    crate::diff::fnv1a64_hex(&material)
}

/// The status walk alone — the repository's state, its changed paths, and each
/// path's content key — with the key that names all of it. No line is counted
/// and no patch is rendered here.
pub(crate) fn status_shape(repo_path: &Path, max_files: usize) -> Result<(Value, String), String> {
    let mut repo = open_repo(repo_path)?;
    let branch = current_branch(&repo)?;
    let head = head_commit_id(&repo)?.map(|oid| oid.to_string());
    let repo_state = repo_state_label(&repo)?;
    let (upstream, ahead, behind) = upstream_status(&repo);
    let (files, files_truncated) = status_files(&repo, max_files)?;
    let stash_count = count_stashes(&mut repo)?;
    let shape = json!({
        "branch": branch,
        "path": repo_path.display().to_string(),
        "head": head,
        "repo_state": repo_state,
        "upstream": upstream,
        "ahead": ahead,
        "behind": behind,
        "stash_count": stash_count,
        "files": files,
        "files_truncated": files_truncated,
    });
    let key = status_key(&shape);
    Ok((shape, key))
}

/// Every changed path, by path, each carrying its staging tri-state and its
/// content key; `max_files` is injectable so tests exercise the truncation
/// path without a 2 000-file fixture.
///
/// Rename detection stays OFF: a staged rename decomposes into a plain
/// D (old path) + A (new path) pair, matching the patch (which has no
/// rename detection) and keeping stage/unstage per-path symmetric. With
/// renames on, git2 reports one "R" entry under the OLD path only — the
/// new path never surfaces and unstaging the row half-unstages the rename.
fn status_files(repo: &git2::Repository, max_files: usize) -> Result<(Vec<Value>, bool), String> {
    let keys = ContentKeys::of(repo);
    let worktree_root = repo.workdir().unwrap_or_else(|| repo.path());
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    let mut files: Vec<Value> = statuses
        .iter()
        .filter_map(|entry| {
            let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
            if crate::diff::is_mcp_config(&path) {
                return None;
            }
            let mut file = file_status_json(&path, entry.status())?;
            file["content_key"] = json!(keys.key_for(&path, &entry));
            if let Some(edited_at) = crate::diff::file_edited_at(worktree_root, &path) {
                file["edited_at"] = json!(edited_at);
            }
            Some(file)
        })
        .collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let files_truncated = files.len() > max_files;
    files.truncate(max_files);
    Ok((files, files_truncated))
}

/// The walk AND the census, for a caller that needs the key beside them —
/// the push bus, which carries `status_key` as its own field.
///
/// The census is a second diff over the working tree. When it cannot be
/// taken the shape alone is answered, so a flush still carries the walk it
/// already paid for rather than nothing at all.
pub(crate) fn counted_status_shape(
    repo_path: &Path,
    max_files: usize,
) -> Result<(Value, String), String> {
    let (shape, key) = status_shape(repo_path, max_files)?;
    let counted = match crate::diff::uncommitted_file_deltas(repo_path) {
        Ok(deltas) => with_line_counts(shape, &deltas, key.clone()),
        Err(_) => shape,
    };
    Ok((counted, key))
}

/// The shape with the working tree's line census joined in.
fn counted_status(shape: Value, status_key: String, repo_path: &Path) -> Result<Value, String> {
    let deltas = crate::diff::uncommitted_file_deltas(repo_path).map_err(|e| e.to_string())?;
    Ok(with_line_counts(shape, &deltas, status_key))
}

/// Each file's added/deleted/binary, and the `stat` totals those files sum to
/// — one census, joined onto the shape it describes.
fn with_line_counts(shape: Value, deltas: &[crate::diff::FileDelta], status_key: String) -> Value {
    let counted: HashMap<&str, &crate::diff::FileDelta> = deltas
        .iter()
        .map(|delta| (delta.path.as_str(), delta))
        .collect();
    let mut payload = shape;
    for file in payload["files"].as_array_mut().into_iter().flatten() {
        let delta = file["path"].as_str().and_then(|path| counted.get(path));
        file["added"] = json!(delta.map_or(0, |delta| delta.added));
        file["deleted"] = json!(delta.map_or(0, |delta| delta.deleted));
        file["binary"] = json!(delta.is_some_and(|delta| delta.binary));
    }
    payload["stat"] = crate::diff::DiffStat {
        files_changed: deltas.len(),
        insertions: deltas.iter().map(|delta| delta.added).sum(),
        deletions: deltas.iter().map(|delta| delta.deleted).sum(),
    }
    .to_json();
    payload["status_key"] = json!(status_key);
    payload
}

/// [`status_payload`] with the `files` cap injectable, so tests exercise the
/// truncation path without a 2 000-file fixture.
#[cfg(test)]
pub(super) fn status_payload_with_file_cap(
    repo_path: &Path,
    max_files: usize,
) -> Result<Value, String> {
    let (shape, key) = status_shape(repo_path, max_files)?;
    counted_status(shape, key, repo_path)
}
