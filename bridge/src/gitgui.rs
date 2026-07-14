//! Git helpers for the browser git-GUI RPCs (`git.log`, `git.show`,
//! `git.status`, `git.stage`, `git.unstage`, `git.commit`).
//!
//! The split mirrors the rest of the bridge: reads go through `git2`,
//! mutations shell out to the `git` binary as plain argv (never a shell),
//! always with the `--` guard before positional paths. Every client-supplied
//! path is fenced lexically with [`crate::task::is_worktree_contained_path`]
//! before any git call, and the scaffolded `.build/mcp.json` never enters the
//! index through these verbs.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{json, Value};

/// Cap on the `patch` field of `git.show` / `git.status`, mirroring
/// `FS_READ_MAX_BYTES`: relay WS frames cap at 8 MiB, so a huge commit must
/// degrade (truncate + flag) instead of resetting the connection.
pub const GIT_SHOW_MAX_PATCH_BYTES: usize = 1_048_576;

/// Cap on the `files` array of the status payload — same relay-frame
/// rationale as [`GIT_SHOW_MAX_PATCH_BYTES`]: a huge untracked tree (say a
/// fresh checkout with no .gitignore and a node_modules/) must degrade to
/// the first N entries plus a `files_truncated` flag, not blow past the
/// 8 MiB WS frame cap and reset the connection on every poll.
pub const GIT_STATUS_MAX_FILES: usize = 2_000;

/// Cap on the commit `subject` display string (git.log rides the 1.6 s poll,
/// and git places no limit on message size). Truncated at a UTF-8 boundary;
/// silent — no wire flag for display strings.
pub const GIT_SUBJECT_MAX_BYTES: usize = 512;

/// Cap on the commit `body` display string of `git.show`, same rationale as
/// [`GIT_SUBJECT_MAX_BYTES`].
pub const GIT_BODY_MAX_BYTES: usize = 65_536;

fn open_repo(repo_path: &Path) -> Result<git2::Repository, String> {
    git2::Repository::open(repo_path).map_err(|e| format!("cannot open repository: {e}"))
}

/// Whether a `repo.head()` error means "no commits yet" rather than a broken repo.
fn is_unborn_head_error(error: &git2::Error) -> bool {
    matches!(
        error.code(),
        git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
    )
}

/// The checked-out branch name: HEAD's shorthand, or — on an unborn HEAD —
/// the shorthand of the branch HEAD symbolically points at.
fn current_branch(repo: &git2::Repository) -> Result<String, String> {
    match repo.head() {
        Ok(head) => Ok(head.shorthand().unwrap_or("HEAD").to_string()),
        Err(e) if is_unborn_head_error(&e) => {
            let head_ref = repo
                .find_reference("HEAD")
                .map_err(|e| format!("cannot read HEAD: {e}"))?;
            let target = head_ref.symbolic_target().unwrap_or("HEAD");
            Ok(target
                .strip_prefix("refs/heads/")
                .unwrap_or(target)
                .to_string())
        }
        Err(e) => Err(format!("cannot read HEAD: {e}")),
    }
}

/// HEAD's commit id, or `None` when HEAD is unborn.
fn head_commit_id(repo: &git2::Repository) -> Result<Option<git2::Oid>, String> {
    match repo.head() {
        Ok(head) => Ok(Some(
            head.peel_to_commit()
                .map_err(|e| format!("cannot resolve HEAD: {e}"))?
                .id(),
        )),
        Err(e) if is_unborn_head_error(&e) => Ok(None),
        Err(e) => Err(format!("cannot read HEAD: {e}")),
    }
}

/// The wire summary shared by `git.log` entries and `git.show`/`git.commit`.
fn commit_summary_json(commit: &git2::Commit) -> Value {
    let hash = commit.id().to_string();
    let (subject, _) = truncate_at_utf8_boundary(
        commit.summary().unwrap_or("").to_string(),
        GIT_SUBJECT_MAX_BYTES,
    );
    json!({
        "short": hash[..7],
        "hash": hash,
        "subject": subject,
        "author": commit.author().name().unwrap_or("").to_string(),
        "email": commit.author().email().unwrap_or("").to_string(),
        "time": commit.time().seconds(),
    })
}

/// The set of commits reachable from HEAD but not from `base_branch` — the
/// "ahead of base" marker set for task-scoped `git.log`.
fn commits_ahead_of(
    repo: &git2::Repository,
    base_branch: &str,
) -> Result<HashSet<git2::Oid>, String> {
    let base_tip = repo
        .revparse_single(base_branch)
        .map_err(|e| format!("cannot resolve base branch {base_branch}: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("base branch {base_branch} is not a commit: {e}"))?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    walk.hide(base_tip.id()).map_err(|e| e.to_string())?;
    walk.map(|oid| oid.map_err(|e| e.to_string())).collect()
}

/// One page of commit history from HEAD, topological newest-first. With
/// `mark_ahead_of` (task scope), each entry carries `ahead_of_base`; without
/// it (project scope) the field is omitted entirely.
pub fn log_page(
    repo_path: &Path,
    mark_ahead_of: Option<&str>,
    limit: usize,
    skip: usize,
) -> Result<Value, String> {
    let repo = open_repo(repo_path)?;
    let branch = current_branch(&repo)?;
    if head_commit_id(&repo)?.is_none() {
        return Ok(json!({ "branch": branch, "commits": [], "more": false }));
    }
    let ahead_set = match mark_ahead_of {
        Some(base_branch) => Some(commits_ahead_of(&repo, base_branch)?),
        None => None,
    };
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    let mut page = walk.skip(skip);
    let mut commits = Vec::with_capacity(limit);
    for _ in 0..limit {
        let Some(oid) = page.next().transpose().map_err(|e| e.to_string())? else {
            break;
        };
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let mut entry = commit_summary_json(&commit);
        if let Some(ahead) = &ahead_set {
            entry["ahead_of_base"] = json!(ahead.contains(&oid));
        }
        commits.push(entry);
    }
    let more = page
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .is_some();
    Ok(json!({ "branch": branch, "commits": commits, "more": more }))
}

/// Whether `hash` is an acceptable `git.show` argument: 4–40 lowercase hex
/// characters — an object-id prefix, never a general revspec.
fn is_valid_hash_prefix(hash: &str) -> bool {
    (4..=40).contains(&hash.len()) && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Resolve a validated hex prefix to a commit via the object database —
/// deliberately not `revparse`, so a branch that happens to be named like hex
/// can never shadow an object id.
fn resolve_commit_prefix<'repo>(
    repo: &'repo git2::Repository,
    hash: &str,
) -> Result<git2::Commit<'repo>, String> {
    let padded = format!("{hash:0<40}");
    let prefix = git2::Oid::from_str(&padded).map_err(|e| format!("invalid hash: {e}"))?;
    let odb = repo.odb().map_err(|e| e.to_string())?;
    let full = odb
        .exists_prefix(prefix, hash.len())
        .map_err(|_| format!("unknown or ambiguous commit: {hash}"))?;
    repo.find_commit(full)
        .map_err(|_| format!("not a commit: {hash}"))
}

/// Unified patch + exact counts for `old_tree` → `new_tree` (`None` = empty
/// tree, for root commits). Counting happens during the print walk, same as
/// `diff.rs`, so the numbers always match the (pre-truncation) patch.
fn tree_diff_patch(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
    new_tree: &git2::Tree,
) -> Result<(Value, String), String> {
    let diff = repo
        .diff_tree_to_tree(old_tree, Some(new_tree), None)
        .map_err(|e| e.to_string())?;
    let files_changed = diff.deltas().len();
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            '+' => insertions += 1,
            '-' => deletions += 1,
            _ => {}
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin());
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| e.to_string())?;
    let stat = json!({
        "files_changed": files_changed,
        "insertions": insertions,
        "deletions": deletions,
    });
    Ok((stat, patch))
}

/// Truncate `text` to at most `max_bytes`, cutting back to a UTF-8 boundary.
/// Returns the (possibly shortened) text and whether truncation happened.
pub fn truncate_at_utf8_boundary(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut cut = max_bytes;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = text;
    truncated.truncate(cut);
    (truncated, true)
}

/// `git.show`: one commit's metadata, exact stat, and (capped) patch against
/// its first parent — the empty tree for a root commit.
pub fn show_commit(repo_path: &Path, hash: &str) -> Result<Value, String> {
    if !is_valid_hash_prefix(hash) {
        return Err("invalid hash: expected 4-40 lowercase hex characters".to_string());
    }
    let repo = open_repo(repo_path)?;
    let commit = resolve_commit_prefix(&repo, hash)?;
    let commit_tree = commit.tree().map_err(|e| e.to_string())?;
    let parent_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(
            commit
                .parent(0)
                .map_err(|e| e.to_string())?
                .tree()
                .map_err(|e| e.to_string())?,
        )
    };
    let (stat, patch) = tree_diff_patch(&repo, parent_tree.as_ref(), &commit_tree)?;
    let (patch, truncated) = truncate_at_utf8_boundary(patch, GIT_SHOW_MAX_PATCH_BYTES);
    let mut result = commit_summary_json(&commit);
    let (body, _) =
        truncate_at_utf8_boundary(commit.body().unwrap_or("").to_string(), GIT_BODY_MAX_BYTES);
    result["body"] = json!(body);
    result["stat"] = stat;
    result["patch"] = json!(patch);
    result["truncated"] = json!(truncated);
    Ok(result)
}

/// Per-file staging tri-state + index/worktree letters for one statuses entry.
/// `None` for entries that carry no reportable change (e.g. ignored).
fn file_status_json(path: &str, status: git2::Status) -> Option<Value> {
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
    let index_changed = status.intersects(
        git2::Status::INDEX_NEW
            | git2::Status::INDEX_MODIFIED
            | git2::Status::INDEX_DELETED
            | git2::Status::INDEX_RENAMED
            | git2::Status::INDEX_TYPECHANGE,
    );
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
pub fn status_payload(repo_path: &Path) -> Result<Value, String> {
    status_payload_with_file_cap(repo_path, GIT_STATUS_MAX_FILES)
}

/// [`status_payload`] with the `files` cap injectable, so tests exercise the
/// truncation path without a 2 000-file fixture.
fn status_payload_with_file_cap(repo_path: &Path, max_files: usize) -> Result<Value, String> {
    let repo = open_repo(repo_path)?;
    let branch = current_branch(&repo)?;
    let head = head_commit_id(&repo)?.map(|oid| oid.to_string());
    // Rename detection stays OFF: a staged rename decomposes into a plain
    // D (old path) + A (new path) pair, matching the patch (which has no
    // rename detection) and keeping stage/unstage per-path symmetric. With
    // renames on, git2 reports one "R" entry under the OLD path only — the
    // new path never surfaces and unstaging the row half-unstages the rename.
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    let mut files: Vec<Value> = statuses
        .iter()
        .filter_map(|entry| {
            let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
            if path == crate::diff::MCP_CONFIG_PATH {
                return None;
            }
            file_status_json(&path, entry.status())
        })
        .collect();
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    let files_truncated = files.len() > max_files;
    files.truncate(max_files);
    let diff = crate::diff::diff_uncommitted(repo_path).map_err(|e| e.to_string())?;
    let stat = diff.stat();
    let (patch, truncated) =
        truncate_at_utf8_boundary(diff.patch().to_string(), GIT_SHOW_MAX_PATCH_BYTES);
    Ok(json!({
        "branch": branch,
        "path": repo_path.display().to_string(),
        "head": head,
        "files": files,
        "files_truncated": files_truncated,
        "stat": {
            "files_changed": stat.files_changed,
            "insertions": stat.insertions,
            "deletions": stat.deletions,
        },
        "patch": patch,
        "truncated": truncated,
    }))
}

/// Validate and filter a stage/unstage path list: every path must pass the
/// lexical worktree fence (one bad path fails the whole request, before any
/// git call), and the machine-local MCP config is silently dropped (compared
/// against the raw path, before any pathspec decoration). Each surviving
/// path becomes a `:(literal)` pathspec so git never applies glob semantics
/// to a client-supplied string — a path like `*` is lexically valid but must
/// only ever match a file actually named `*`.
fn stageable_paths(paths: &[String]) -> Result<Vec<String>, String> {
    for path in paths {
        if !crate::task::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
    }
    Ok(paths
        .iter()
        .filter(|path| path.as_str() != crate::diff::MCP_CONFIG_PATH)
        .map(|path| format!(":(literal){path}"))
        .collect())
}

/// Run a git subcommand in `repo_path` as plain argv (no shell), returning
/// stdout. A non-zero exit joins stderr and stdout into the error — git
/// splits its story across both streams.
fn run_git(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            format!("{} {}", stderr.trim(), stdout.trim()).trim()
        ));
    }
    Ok(stdout)
}

/// `git.stage`: `git add -- <paths…>`. An empty surviving list (everything
/// filtered) succeeds as a no-op.
pub fn stage_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    let surviving = stageable_paths(paths)?;
    if surviving.is_empty() {
        return Ok(());
    }
    let mut args = vec!["add", "--"];
    args.extend(surviving.iter().map(String::as_str));
    run_git(repo_path, &args).map(|_| ())
}

/// `git.unstage`: `git reset -q HEAD -- <paths…>`, or — when HEAD is unborn
/// and there is nothing to reset to — drop the index entries with
/// `git rm -f -r -q --cached -- <paths…>`. The `-f` is required whenever the
/// staged copy differs from the worktree copy (on an unborn HEAD "differs
/// from HEAD" is always true), and is safe: `--cached` never touches the
/// worktree file.
pub fn unstage_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    let surviving = stageable_paths(paths)?;
    if surviving.is_empty() {
        return Ok(());
    }
    let repo = open_repo(repo_path)?;
    let mut args = if head_commit_id(&repo)?.is_some() {
        vec!["reset", "-q", "HEAD", "--"]
    } else {
        vec!["rm", "-f", "-r", "-q", "--cached", "--"]
    };
    args.extend(surviving.iter().map(String::as_str));
    run_git(repo_path, &args).map(|_| ())
}

/// `git.commit`: commit exactly what is staged (no `-a`, no auto-stage) with
/// the trimmed message as a single argv element. Returns the new commit's
/// summary (`hash`/`short`/`subject`…).
pub fn commit_staged(repo_path: &Path, message: &str) -> Result<Value, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("commit message must not be empty".to_string());
    }
    let staged = run_git(repo_path, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Err("nothing staged to commit".to_string());
    }
    run_git(repo_path, &["commit", "-m", message])?;
    let repo = open_repo(repo_path)?;
    let head = repo
        .head()
        .map_err(|e| e.to_string())?
        .peel_to_commit()
        .map_err(|e| e.to_string())?;
    Ok(commit_summary_json(&head))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_prefix_validation_accepts_only_lowercase_hex_of_4_to_40() {
        assert!(is_valid_hash_prefix("deadbeef"));
        assert!(is_valid_hash_prefix("0123"));
        assert!(is_valid_hash_prefix(&"a".repeat(40)));
        assert!(!is_valid_hash_prefix("abc")); // too short
        assert!(!is_valid_hash_prefix(&"a".repeat(41))); // too long
        assert!(!is_valid_hash_prefix("DEADBEEF")); // uppercase
        assert!(!is_valid_hash_prefix("deadbeeg")); // non-hex
        assert!(!is_valid_hash_prefix("HEAD~1")); // revspec, not a hash
        assert!(!is_valid_hash_prefix(""));
    }

    #[test]
    fn truncation_respects_utf8_boundaries_and_flags() {
        let (untouched, truncated) = truncate_at_utf8_boundary("short".to_string(), 100);
        assert_eq!(untouched, "short");
        assert!(!truncated);

        // "é" is two bytes; a cap landing mid-char must back off, not panic.
        let (cut, truncated) = truncate_at_utf8_boundary("aé".to_string(), 2);
        assert_eq!(cut, "a");
        assert!(truncated);

        let (exact, truncated) = truncate_at_utf8_boundary("abcd".to_string(), 4);
        assert_eq!(exact, "abcd");
        assert!(!truncated);
    }

    #[test]
    fn stageable_paths_fences_and_filters() {
        let mixed: Vec<String> = vec!["src/a.rs".into(), ".build/mcp.json".into()];
        // Survivors come out as :(literal) pathspecs — glob-proof; the MCP
        // config filter compares against the raw path, before decoration.
        assert_eq!(stageable_paths(&mixed).unwrap(), vec![":(literal)src/a.rs"]);
        assert_eq!(stageable_paths(&["*".into()]).unwrap(), vec![":(literal)*"]);

        assert!(stageable_paths(&["../evil".into()]).is_err());
        assert!(stageable_paths(&["/etc/passwd".into()]).is_err());
        assert!(stageable_paths(&["./relative".into()]).is_err());
        assert!(stageable_paths(&["".into()]).is_err());
        // One bad path poisons the whole list.
        assert!(stageable_paths(&["fine.txt".into(), "../evil".into()]).is_err());
    }

    #[test]
    fn untracked_and_tristate_status_letters() {
        let untracked = file_status_json("new.txt", git2::Status::WT_NEW).unwrap();
        assert_eq!(untracked["staged"], "none");
        assert_eq!(untracked["index_status"], "?");
        assert_eq!(untracked["worktree_status"], "?");

        let full = file_status_json("added.txt", git2::Status::INDEX_NEW).unwrap();
        assert_eq!(full["staged"], "full");
        assert_eq!(full["index_status"], "A");
        assert_eq!(full["worktree_status"], "-");

        let partial = file_status_json(
            "both.txt",
            git2::Status::INDEX_MODIFIED | git2::Status::WT_MODIFIED,
        )
        .unwrap();
        assert_eq!(partial["staged"], "partial");
        assert_eq!(partial["index_status"], "M");
        assert_eq!(partial["worktree_status"], "M");

        let worktree_only = file_status_json("dirty.txt", git2::Status::WT_MODIFIED).unwrap();
        assert_eq!(worktree_only["staged"], "none");
        assert_eq!(worktree_only["index_status"], "-");
        assert_eq!(worktree_only["worktree_status"], "M");

        assert!(file_status_json("clean.txt", git2::Status::CURRENT).is_none());
        assert!(file_status_json("ignored.txt", git2::Status::IGNORED).is_none());
    }

    #[test]
    fn status_files_list_is_capped_with_a_truncation_flag() {
        let dir = tempfile::tempdir().unwrap();
        git2::Repository::init(dir.path()).unwrap();
        for i in 0..8 {
            std::fs::write(dir.path().join(format!("f{i:02}.txt")), "x\n").unwrap();
        }

        // Over the cap: first N by path, flagged; stat/patch stay exact.
        let capped = status_payload_with_file_cap(dir.path(), 5).unwrap();
        let files = capped["files"].as_array().unwrap();
        assert_eq!(files.len(), 5);
        assert_eq!(capped["files_truncated"], true);
        assert_eq!(files[0]["path"], "f00.txt");
        assert_eq!(files[4]["path"], "f04.txt");
        assert_eq!(capped["stat"]["files_changed"], 8);
        assert!(capped["patch"].as_str().unwrap().contains("+x"));

        // At the cap exactly: everything fits, no flag.
        let uncapped = status_payload_with_file_cap(dir.path(), 8).unwrap();
        assert_eq!(uncapped["files"].as_array().unwrap().len(), 8);
        assert_eq!(uncapped["files_truncated"], false);
    }

    #[test]
    fn conflicted_entries_surface_as_u_instead_of_vanishing() {
        // libgit2 reports unmerged entries with CONFLICTED alone — no INDEX_*
        // or WT_* bits — so they must not fall through the changed-bits check.
        let conflicted = file_status_json("f.txt", git2::Status::CONFLICTED).unwrap();
        assert_eq!(conflicted["staged"], "none");
        assert_eq!(conflicted["index_status"], "U");
        assert_eq!(conflicted["worktree_status"], "U");
    }
}
