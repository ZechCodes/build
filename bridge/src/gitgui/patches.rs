use super::history::{open_repo, truncate_at_utf8_boundary, GIT_SHOW_MAX_PATCH_BYTES};
use super::status::ContentKeys;
use crate::body_page::{text_page, BodyRange};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

pub const GIT_DIFF_MAX_PATHS: usize = 50;

/// The most bytes of patch one `git.diff` answer may carry, across all its
/// files. The per-file cap alone does not bound the answer:
/// [`GIT_DIFF_MAX_PATHS`] files at [`GIT_SHOW_MAX_PATCH_BYTES`] each is
/// 50 MiB, and the DataChannel refuses a reassembly over `MAX_REASSEMBLED_BYTES`
/// (8 MiB, `rtc/chunk.rs`) — which base64-after-encryption reaches at roughly
/// 6 MiB of plaintext. The budget is spent in request order.
pub const GIT_DIFF_MAX_ANSWER_BYTES: usize = 4 * 1_048_576;

/// `git.diff` — the uncommitted patch of each of `paths`, with the content key
/// each body should be cached under.
///
/// Answers in request order, one entry per asked path: a path with no change
/// answers an empty patch and its key, each patch is capped at
/// [`GIT_SHOW_MAX_PATCH_BYTES`], and the answer as a whole is capped at
/// [`GIT_DIFF_MAX_ANSWER_BYTES`] — a shortened patch carries `truncated`.
pub fn file_patches(repo_path: &Path, paths: &[String]) -> Result<Value, String> {
    file_patches_capped(
        repo_path,
        paths,
        GIT_SHOW_MAX_PATCH_BYTES,
        GIT_DIFF_MAX_ANSWER_BYTES,
    )
}

/// `git.diff` with a `range` (#95): one page of ONE path's patch, under the
/// same content key the whole read answers. The per-file and per-answer caps
/// give way to the page's own.
pub fn file_patch_page(
    repo_path: &Path,
    paths: &[String],
    range: BodyRange,
) -> Result<Value, String> {
    reject_unreadable_paths(paths)?;
    let [path] = paths else {
        return Err("git.diff takes one path with a range".to_string());
    };
    let repo = open_repo(repo_path)?;
    let keys = content_keys_for_paths(&repo, paths)?;
    let patch = crate::diff::patch_for_paths(repo_path, paths)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|file| &file.path == path)
        .map(|file| file.patch)
        .unwrap_or_default();
    let (page, span) = text_page(&patch, range)?;
    Ok(json!({ "files": [{
        "path": path,
        "content_key": keys.get(path).cloned().unwrap_or_default(),
        "patch": page,
        "truncated": false,
        "range": span,
    }] }))
}

/// [`file_patches`] with both caps injectable, so a test exercises the
/// truncation paths without megabytes of fixture.
pub(super) fn file_patches_capped(
    repo_path: &Path,
    paths: &[String],
    max_patch_bytes: usize,
    max_answer_bytes: usize,
) -> Result<Value, String> {
    reject_unreadable_paths(paths)?;
    let repo = open_repo(repo_path)?;
    let keys = content_keys_for_paths(&repo, paths)?;
    let rendered: HashMap<String, String> = crate::diff::patch_for_paths(repo_path, paths)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|file| (file.path, file.patch))
        .collect();
    let mut unspent = max_answer_bytes;
    let mut files: Vec<Value> = Vec::with_capacity(paths.len());
    for path in paths {
        let (patch, truncated) = truncate_at_utf8_boundary(
            rendered.get(path).cloned().unwrap_or_default(),
            max_patch_bytes.min(unspent),
        );
        unspent -= patch.len();
        files.push(json!({
            "path": path,
            "content_key": keys.get(path).cloned().unwrap_or_default(),
            "patch": patch,
            "truncated": truncated,
        }));
    }
    Ok(json!({ "files": files }))
}

/// The fence `git.diff` reads paths through. It is the pair of predicates
/// [`stageable_paths`] is built from, but both are fatal here: `git.diff`
/// answers every path it is asked for, in order, so a path it cannot read is
/// an error rather than a dropped answer nobody can align.
fn reject_unreadable_paths(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() || paths.len() > GIT_DIFF_MAX_PATHS {
        return Err(format!(
            "git.diff takes 1 to {GIT_DIFF_MAX_PATHS} paths, got {}",
            paths.len()
        ));
    }
    for path in paths {
        if !crate::plan::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
        if crate::diff::is_mcp_config(path) {
            return Err(format!("path is not readable through git.diff: {path}"));
        }
    }
    Ok(())
}

/// The content key of each of `paths`, from a status walk restricted to them.
/// A path the walk does not report is unchanged, and keys off its file on disk.
fn content_keys_for_paths(
    repo: &git2::Repository,
    paths: &[String],
) -> Result<HashMap<String, String>, String> {
    let keys = ContentKeys::of(repo);
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .disable_pathspec_match(true);
    for path in paths {
        opts.pathspec(path);
    }
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    let mut found: HashMap<String, String> = statuses
        .iter()
        .map(|entry| {
            let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
            let key = keys.key_for(&path, &entry);
            (path, key)
        })
        .collect();
    for path in paths {
        found
            .entry(path.clone())
            .or_insert_with(|| keys.key_off_disk(path));
    }
    Ok(found)
}
