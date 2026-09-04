//! Git helpers for the browser git-GUI RPCs (`git.log`, `git.show`,
//! `git.status`, `git.stage`, `git.unstage`, `git.commit`).
//!
//! The split mirrors the rest of the bridge: reads go through `git2`,
//! mutations shell out to the `git` binary as plain argv (never a shell),
//! always with the `--` guard before positional paths. Every client-supplied
//! path is fenced lexically with [`crate::plan::is_worktree_contained_path`]
//! before any git call, and the scaffolded `.build/mcp.json` never enters the
//! index through these verbs.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Hard wall-clock cap on any network git subprocess (fetch/pull/push). The
/// global AppState mutex is held for the whole RPC, so a hung network op would
/// freeze every poll; on expiry the child is killed and the op returns an
/// error. Paired with `GIT_TERMINAL_PROMPT=0`, which makes git fail fast on a
/// credential prompt instead of blocking on stdin.
pub const GIT_NETWORK_TIMEOUT_SECS: u64 = 60;

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
/// "ahead of base" marker set for run-scoped `git.log`. Anchored on the base
/// branch's fork point (not the run's `base_sha`), so the materialized-plan
/// commit and every build commit after it all read as ahead on the commit rail.
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
/// `mark_ahead_of` (run scope), each entry carries `ahead_of_base`; without
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

/// Whether a status carries any staged (index-side) change — the shared
/// predicate behind the staging tri-state and the discard tracked/untracked
/// split, so both agree on what "already in the index" means.
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
fn is_untracked_status(status: git2::Status) -> bool {
    status.contains(git2::Status::WT_NEW) && !has_index_change(status)
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
pub fn status_payload(repo_path: &Path) -> Result<Value, String> {
    status_payload_with_file_cap(repo_path, GIT_STATUS_MAX_FILES)
}

/// Map a raw [`git2::RepositoryState`] to the wire vocabulary the SPA banner
/// keys on, before the working-tree conflict check: any rebase flavor is
/// "rebasing", a merge is "merging", a cherry-pick / revert / bisect maps to
/// its own label, a mailbox-apply (and anything future) is "other", and an
/// idle repo is "clean".
fn map_repository_state(state: git2::RepositoryState) -> &'static str {
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
fn repo_state_label(repo: &git2::Repository) -> Result<&'static str, String> {
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
fn upstream_status(repo: &git2::Repository) -> (Option<String>, Option<u64>, Option<u64>) {
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

/// [`status_payload`] with the `files` cap injectable, so tests exercise the
/// truncation path without a 2 000-file fixture.
fn status_payload_with_file_cap(repo_path: &Path, max_files: usize) -> Result<Value, String> {
    let mut repo = open_repo(repo_path)?;
    let branch = current_branch(&repo)?;
    let head = head_commit_id(&repo)?.map(|oid| oid.to_string());
    let repo_state = repo_state_label(&repo)?;
    let (upstream, ahead, behind) = upstream_status(&repo);
    // Rename detection stays OFF: a staged rename decomposes into a plain
    // D (old path) + A (new path) pair, matching the patch (which has no
    // rename detection) and keeping stage/unstage per-path symmetric. With
    // renames on, git2 reports one "R" entry under the OLD path only — the
    // new path never surfaces and unstaging the row half-unstages the rename.
    // Scoped so the immutable statuses borrow ends before the &mut stash walk.
    let (files, files_truncated) = {
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
                file_status_json(&path, entry.status())
            })
            .collect();
        files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
        let files_truncated = files.len() > max_files;
        files.truncate(max_files);
        (files, files_truncated)
    };
    let stash_count = count_stashes(&mut repo)?;
    let diff = crate::diff::diff_uncommitted(repo_path).map_err(|e| e.to_string())?;
    let stat = diff.stat();
    let (patch, truncated) =
        truncate_at_utf8_boundary(diff.patch().to_string(), GIT_SHOW_MAX_PATCH_BYTES);
    Ok(json!({
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
        "stat": stat.to_json(),
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
        if !crate::plan::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
    }
    Ok(paths
        .iter()
        .filter(|path| !crate::diff::is_mcp_config(path.as_str()))
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

/// Run a prepared command with a hard wall-clock cap, draining stdout/stderr
/// on threads so a chatty child can never dead-lock on a full pipe while we
/// poll. On expiry the child is killed and `<op> timed out after <n>s` is
/// returned; a non-zero exit joins stderr+stdout into the error, matching
/// [`run_git`]. `op_label` names the operation for both messages.
fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    op_label: &str,
) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run {op_label}: {e}"))?;
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| format!("could not wait for {op_label}: {e}"))?
        {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{op_label} timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let stdout = String::from_utf8_lossy(&stdout_reader.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_reader.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(format!(
            "{op_label} failed: {}",
            format!("{} {}", stderr.trim(), stdout.trim()).trim()
        ));
    }
    Ok(stdout)
}

/// Run a network git subcommand (fetch/pull/push) with `GIT_TERMINAL_PROMPT=0`
/// (fail fast on a credential prompt, never block on stdin) under the
/// [`GIT_NETWORK_TIMEOUT_SECS`] wall-clock cap.
fn run_git_network(repo_path: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let op_label = format!("git {}", args.first().unwrap_or(&""));
    run_with_timeout(
        command,
        Duration::from_secs(GIT_NETWORK_TIMEOUT_SECS),
        &op_label,
    )
}

/// `git.fetch`: `git fetch --prune`. Safe in every scope — it only updates
/// remote-tracking refs, never the working tree.
pub fn fetch(repo_path: &Path) -> Result<(), String> {
    run_git_network(repo_path, &["fetch", "--prune"]).map(|_| ())
}

/// `git.pull` mode → the matching git flag. Unknown modes fail fast rather
/// than silently defaulting.
fn pull_flag(mode: &str) -> Result<&'static str, String> {
    match mode {
        "ff" => Ok("--ff-only"),
        "merge" => Ok("--no-rebase"),
        "rebase" => Ok("--rebase"),
        other => Err(format!("unknown pull mode: {other}")),
    }
}

/// `git.pull`: `git pull` in the requested integration mode. Git's own message
/// passes through verbatim on failure (ff-only refusal, conflict); a conflicted
/// pull deliberately leaves the repo in a merging/rebasing state, which the
/// next `git.status` surfaces.
pub fn pull(repo_path: &Path, mode: &str) -> Result<(), String> {
    let flag = pull_flag(mode)?;
    run_git_network(repo_path, &["pull", flag]).map(|_| ())
}

/// `git.push`: `git push` (or `git push -u origin -- <branch>` when the branch
/// has no upstream yet). `force` upgrades to `--force-with-lease` — never a
/// bare `--force`. A detached HEAD has no branch to push and is refused.
pub fn push(repo_path: &Path, force: bool) -> Result<(), String> {
    let repo = open_repo(repo_path)?;
    if repo
        .head_detached()
        .map_err(|e| format!("cannot read HEAD: {e}"))?
    {
        return Err("cannot push a detached HEAD".to_string());
    }
    let branch = current_branch(&repo)?;
    let (upstream, _, _) = upstream_status(&repo);
    drop(repo);
    let mut args: Vec<String> = vec!["push".to_string()];
    if force {
        args.push("--force-with-lease".to_string());
    }
    if upstream.is_none() {
        args.push("-u".to_string());
        args.push("origin".to_string());
        args.push("--".to_string());
        args.push(branch);
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_git_network(repo_path, &arg_refs).map(|_| ())
}

/// Where the ref behind a branch name lives: the repository's own
/// `refs/heads/<name>`, a remote's `refs/remotes/<remote>/<name>` for a branch
/// that has never been checked out here, or nowhere at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchOrigin {
    Local,
    Remote {
        remote: String,
        tracking_ref: String,
    },
    Absent,
}

impl BranchOrigin {
    /// The full ref name a branch of this origin is read through — never the
    /// bare branch name, which git would resolve as a revspec. A branch with
    /// no ref yet is named by the local ref it would be cut as.
    fn ref_name(&self, branch: &str) -> String {
        match self {
            BranchOrigin::Local | BranchOrigin::Absent => format!("refs/heads/{branch}"),
            BranchOrigin::Remote { tracking_ref, .. } => tracking_ref.clone(),
        }
    }

    /// The remote a branch that exists only there would be fetched from.
    fn remote(&self) -> Option<&str> {
        match self {
            BranchOrigin::Local | BranchOrigin::Absent => None,
            BranchOrigin::Remote { remote, .. } => Some(remote),
        }
    }
}

/// The reference `name` names, when one is there and is a branch.
///
/// A symbolic reference is a pointer at a branch, not a branch: every clone
/// carries `refs/remotes/origin/HEAD`, and treating it as one would offer a
/// `HEAD` branch no remote can serve. It reads here exactly as a missing ref
/// does, so the listing and [`branch_origin`] never disagree about what a
/// remote branch is.
fn direct_reference<'repo>(
    repo: &'repo git2::Repository,
    name: &str,
) -> Result<Option<git2::Reference<'repo>>, git2::Error> {
    match repo.find_reference(name) {
        Ok(reference) if reference.kind() == Some(git2::ReferenceType::Direct) => {
            Ok(Some(reference))
        }
        Ok(_) => Ok(None),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Where the ref behind `branch` lives — the one answer to "is this branch
/// here, on a remote, or nowhere", which decides whether a checkout needs a
/// fetch first and whether teardown owns the branch afterwards.
///
/// Only a missing ref means [`BranchOrigin::Absent`]. Every other git failure
/// is returned: callers guarantee `branch` is a name git could hold a branch
/// under, so a spec error here is a bug and not an answer.
pub fn branch_origin(repo: &git2::Repository, branch: &str) -> Result<BranchOrigin, git2::Error> {
    if direct_reference(repo, &format!("refs/heads/{branch}"))?.is_some() {
        return Ok(BranchOrigin::Local);
    }
    for remote in remotes_in_fetch_precedence(repo)? {
        let tracking_ref = format!("refs/remotes/{remote}/{branch}");
        if direct_reference(repo, &tracking_ref)?.is_some() {
            return Ok(BranchOrigin::Remote {
                remote,
                tracking_ref,
            });
        }
    }
    Ok(BranchOrigin::Absent)
}

/// One branch's git facts for [`branch_list`]: where its ref lives, the sync
/// chips against its own upstream (unrelated to `base_branch`, which is what
/// `stat` is measured from), and its own weight — the diffstat a reviewer
/// would see switching onto it, whether or not it is presently checked out.
pub struct BranchRow {
    pub name: String,
    pub is_current: bool,
    pub origin: BranchOrigin,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub head_subject: String,
    pub head_time: i64,
    pub stat: crate::diff::DiffStat,
}

impl BranchRow {
    /// The row's git facts as a wire object, for a caller that merges its own
    /// stamps into it.
    pub fn into_json(self) -> Value {
        json!({
            "name": self.name,
            "is_current": self.is_current,
            "remote": self.origin.remote(),
            "upstream": self.upstream,
            "ahead": self.ahead,
            "behind": self.behind,
            "head_subject": self.head_subject,
            "head_time": self.head_time,
            "stat": self.stat.to_json(),
        })
    }
}

/// The repository's branches and which one its checkout is on.
pub struct BranchListing {
    pub current: String,
    pub rows: Vec<BranchRow>,
}

/// A local branch's position against its own upstream. A branch with no
/// upstream is level with nothing.
#[derive(Default)]
struct BranchSync {
    upstream: Option<String>,
    ahead: u64,
    behind: u64,
}

fn upstream_sync(
    repo: &git2::Repository,
    branch: &git2::Branch,
    head: &git2::Commit,
) -> Result<BranchSync, String> {
    let Ok(upstream) = branch.upstream() else {
        return Ok(BranchSync::default());
    };
    let name = upstream.name().ok().flatten().map(str::to_string);
    let (ahead, behind) = match upstream.get().target() {
        Some(upstream_oid) => repo
            .graph_ahead_behind(head.id(), upstream_oid)
            .map_err(|e| e.to_string())?,
        None => (0, 0),
    };
    Ok(BranchSync {
        upstream: name,
        ahead: ahead as u64,
        behind: behind as u64,
    })
}

fn branch_row(
    repo_path: &Path,
    base_branch: &str,
    name: String,
    origin: BranchOrigin,
    is_current: bool,
    sync: BranchSync,
    head: &git2::Commit,
) -> Result<BranchRow, String> {
    let (head_subject, _) = truncate_at_utf8_boundary(
        head.summary().unwrap_or("").to_string(),
        GIT_SUBJECT_MAX_BYTES,
    );
    let stat =
        crate::diff::stat_branch_against_base(repo_path, &origin.ref_name(&name), base_branch)
            .map_err(|e| e.to_string())?;
    Ok(BranchRow {
        name,
        is_current,
        origin,
        upstream: sync.upstream,
        ahead: sync.ahead,
        behind: sync.behind,
        head_subject,
        head_time: head.time().seconds(),
        stat,
    })
}

fn local_branch_row(
    repo: &git2::Repository,
    repo_path: &Path,
    base_branch: &str,
    branch: &git2::Branch,
) -> Result<BranchRow, String> {
    let name = branch
        .name()
        .map_err(|e| e.to_string())?
        .unwrap_or("")
        .to_string();
    let head = branch.get().peel_to_commit().map_err(|e| e.to_string())?;
    let sync = upstream_sync(repo, branch, &head)?;
    branch_row(
        repo_path,
        base_branch,
        name,
        BranchOrigin::Local,
        branch.is_head(),
        sync,
        &head,
    )
}

/// The order a branch that exists only on remotes is fetched from: `origin`
/// when the repository has one, then the rest in git's own (sorted) order.
fn remotes_in_fetch_precedence(repo: &git2::Repository) -> Result<Vec<String>, git2::Error> {
    let remotes = repo.remotes()?;
    let mut names: Vec<String> = remotes.iter().flatten().map(str::to_string).collect();
    names.sort_by_key(|name| name != "origin");
    Ok(names)
}

/// The branches one remote carries that no earlier pass has listed.
///
/// Two refs under `refs/remotes/<remote>/` are not branches and never become
/// rows: a symbolic one (every clone has `origin/HEAD`, a pointer at another
/// branch), and one whose name is already listed — by a local branch, whose
/// upstream that ref is and whose own row already carries it, or by a remote
/// earlier in fetch precedence, which is where a fetch would come from.
///
/// Every other ref under that prefix is a branch of this remote, named by the
/// suffix — so a remote whose own name contains a slash still splits where
/// its refs say it does, which git2's remote-branch shorthand cannot promise.
fn remote_branch_rows(
    repo: &git2::Repository,
    repo_path: &Path,
    base_branch: &str,
    remote: &str,
    listed: &HashSet<String>,
) -> Result<Vec<BranchRow>, String> {
    let prefix = format!("refs/remotes/{remote}/");
    let mut rows = Vec::new();
    for reference in repo
        .references_glob(&format!("{prefix}*"))
        .map_err(|e| e.to_string())?
    {
        let reference = reference.map_err(|e| e.to_string())?;
        if reference.kind() != Some(git2::ReferenceType::Direct) {
            continue;
        }
        let Some(tracking_ref) = reference.name() else {
            continue;
        };
        let Some(name) = tracking_ref.strip_prefix(&prefix) else {
            continue;
        };
        if listed.contains(name) {
            continue;
        }
        let head = reference.peel_to_commit().map_err(|e| e.to_string())?;
        rows.push(branch_row(
            repo_path,
            base_branch,
            name.to_string(),
            BranchOrigin::Remote {
                remote: remote.to_string(),
                tracking_ref: tracking_ref.to_string(),
            },
            false,
            BranchSync::default(),
            &head,
        )?);
    }
    Ok(rows)
}

/// `git.branches`: every branch the repository can offer, once each — its
/// local branches, plus the branches its remotes carry that have no local ref
/// yet, each named by the local branch it would become. Current first, then by
/// most-recent head commit time. Pure git2 reads — no working-tree mutation.
pub fn branch_list(repo_path: &Path, base_branch: &str) -> Result<BranchListing, String> {
    let repo = open_repo(repo_path)?;
    let current = current_branch(&repo)?;
    let mut rows = Vec::new();
    for item in repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?
    {
        let (branch, _) = item.map_err(|e| e.to_string())?;
        rows.push(local_branch_row(&repo, repo_path, base_branch, &branch)?);
    }
    let mut listed: HashSet<String> = rows.iter().map(|row| row.name.clone()).collect();
    for remote in remotes_in_fetch_precedence(&repo).map_err(|e| e.to_string())? {
        let remote_rows = remote_branch_rows(&repo, repo_path, base_branch, &remote, &listed)?;
        listed.extend(remote_rows.iter().map(|row| row.name.clone()));
        rows.extend(remote_rows);
    }
    rows.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then_with(|| b.head_time.cmp(&a.head_time))
    });
    Ok(BranchListing { current, rows })
}

/// Reject a client-supplied branch name before it reaches an argv slot: an
/// explicit leading-dash guard (so it can never be read as a flag even where
/// git accepts no `--`, e.g. `git switch`) plus `git check-ref-format
/// --branch`, git's own ref-name grammar.
fn validate_branch_name(repo_path: &Path, branch: &str) -> Result<(), String> {
    if branch.is_empty() || branch.starts_with('-') {
        return Err(format!("invalid branch name: {branch}"));
    }
    run_git(repo_path, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("invalid branch name: {branch}"))?;
    Ok(())
}

/// The reason `git.checkout` refuses to switch branches away from a non-clean
/// state, naming the actual in-progress operation so the message matches what
/// the user must finish or abort. A "conflicted" working tree has no operation
/// to abort — the unmerged files themselves must be resolved.
fn checkout_refusal_message(state: &str) -> String {
    match state {
        "merging" => "finish or abort the in-progress merge first",
        "rebasing" => "finish or abort the in-progress rebase first",
        "cherry-picking" => "finish or abort the in-progress cherry-pick first",
        "reverting" => "finish or abort the in-progress revert first",
        "bisecting" => "finish or abort the in-progress bisect first",
        "conflicted" => "resolve the conflicted files first",
        _ => "finish or abort the in-progress operation first",
    }
    .to_string()
}

/// `git.checkout`: `git switch <branch>` (or `git switch -c <branch>` to
/// create). The name is validated first; the tree may be dirty (git carries
/// the changes or refuses on conflict — either way its message passes through),
/// but any in-progress operation (merge/rebase/cherry-pick/revert/bisect) or a
/// conflicted working tree is refused so the user resolves it first.
pub fn checkout(repo_path: &Path, branch: &str, create: bool) -> Result<(), String> {
    validate_branch_name(repo_path, branch)?;
    let repo = open_repo(repo_path)?;
    let state = repo_state_label(&repo)?;
    drop(repo);
    if state != "clean" {
        return Err(checkout_refusal_message(state));
    }
    let args: Vec<&str> = if create {
        vec!["switch", "-c", branch]
    } else {
        vec!["switch", branch]
    };
    run_git(repo_path, &args).map(|_| ())
}

/// `git.branch_delete`: `git branch -d -- <branch>` (`-D` to force). Deleting
/// the current branch is git's error to raise, and passes through.
pub fn branch_delete(repo_path: &Path, branch: &str, force: bool) -> Result<(), String> {
    validate_branch_name(repo_path, branch)?;
    let flag = if force { "-D" } else { "-d" };
    run_git(repo_path, &["branch", flag, "--", branch]).map(|_| ())
}

/// `git.stash`: `git stash push -u` — include untracked files so the working
/// tree comes back truly clean. An empty tree is git's "No local changes"
/// error, passed through.
pub fn stash_push(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["stash", "push", "-u"]).map(|_| ())
}

/// `git.stash_pop`: `git stash pop`. A pop conflict is git's error; it leaves
/// unmerged (UU) index entries but no MERGE_HEAD, so `repo.state()` stays Clean
/// and the next status surfaces repo_state "conflicted" (not "merging").
pub fn stash_pop(repo_path: &Path) -> Result<(), String> {
    run_git(repo_path, &["stash", "pop"]).map(|_| ())
}

/// `git.merge_abort`: abort whatever operation is in progress with the matching
/// git command — `git merge --abort` while merging, `git rebase --abort` while
/// rebasing, `git cherry-pick --abort` / `git revert --abort` for those, and
/// `git bisect reset` while bisecting. A conflicted-but-idle tree (a stash-pop
/// conflict) or a clean/"other" repo has no operation to abort.
pub fn merge_abort(repo_path: &Path) -> Result<(), String> {
    let repo = open_repo(repo_path)?;
    let state = repo_state_label(&repo)?;
    drop(repo);
    match state {
        "merging" => run_git(repo_path, &["merge", "--abort"]).map(|_| ()),
        "rebasing" => run_git(repo_path, &["rebase", "--abort"]).map(|_| ()),
        "cherry-picking" => run_git(repo_path, &["cherry-pick", "--abort"]).map(|_| ()),
        "reverting" => run_git(repo_path, &["revert", "--abort"]).map(|_| ()),
        "bisecting" => run_git(repo_path, &["bisect", "reset"]).map(|_| ()),
        _ => Err("no abortable operation in progress".to_string()),
    }
}

/// `git.discard` (**destructive**): revert each path to HEAD. Tracked paths go
/// through `git restore --staged --worktree --source=HEAD` (reverting both the
/// index and the working copy); untracked paths are unlinked directly — but
/// only after the two-layer [`crate::app::fenced_scope_path`] guard
/// (lexical + canonical containment), since a symlinked path whose components
/// all look Normal could otherwise resolve outside the worktree. The scaffolded
/// `.build/mcp.json` is silently skipped.
pub fn discard_paths(repo_path: &Path, paths: &[String]) -> Result<(), String> {
    for path in paths {
        if !crate::plan::is_worktree_contained_path(path) {
            return Err(format!("path escapes the worktree: {path}"));
        }
    }
    let repo = open_repo(repo_path)?;
    let mut tracked: Vec<String> = Vec::new();
    let mut untracked: Vec<&String> = Vec::new();
    for path in paths {
        if crate::diff::is_mcp_config(path.as_str()) {
            continue;
        }
        match repo.status_file(Path::new(path)) {
            Ok(status) if is_untracked_status(status) => untracked.push(path),
            _ => tracked.push(format!(":(literal){path}")),
        }
    }
    drop(repo);
    // Untracked deletions first: each is fenced (lexical + canonical) before
    // the unlink, so a traversal or symlink escape can never reach outside.
    for path in untracked {
        let target = crate::app::fenced_scope_path(repo_path, path)?;
        std::fs::remove_file(&target).map_err(|e| format!("cannot delete {path}: {e}"))?;
    }
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--staged", "--worktree", "--source=HEAD", "--"];
        args.extend(tracked.iter().map(String::as_str));
        run_git(repo_path, &args)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    #[test]
    fn untracked_classification_splits_index_from_worktree() {
        // A pristine new file is untracked; once it is in the index (even with
        // a further worktree edit) it is tracked, and a plain worktree edit of
        // a committed file is tracked.
        assert!(is_untracked_status(git2::Status::WT_NEW));
        assert!(!is_untracked_status(
            git2::Status::WT_NEW | git2::Status::INDEX_NEW
        ));
        assert!(!is_untracked_status(git2::Status::WT_MODIFIED));
        assert!(!is_untracked_status(git2::Status::INDEX_MODIFIED));
    }

    #[test]
    fn pull_flag_maps_modes_and_rejects_unknown() {
        assert_eq!(pull_flag("ff").unwrap(), "--ff-only");
        assert_eq!(pull_flag("merge").unwrap(), "--no-rebase");
        assert_eq!(pull_flag("rebase").unwrap(), "--rebase");
        assert!(pull_flag("octopus").is_err());
    }

    #[test]
    fn run_with_timeout_kills_a_child_that_overruns() {
        // A `sleep 5` under a 200 ms cap must be killed and reported, not
        // waited out — this is the network-op backstop's core mechanism.
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();
        let result = run_with_timeout(command, Duration::from_millis(200), "git fetch");
        let elapsed = started.elapsed();
        let error = result.unwrap_err();
        assert!(error.contains("timed out after"), "{error}");
        assert!(error.starts_with("git fetch"), "{error}");
        assert!(
            elapsed < Duration::from_secs(2),
            "killed promptly: {elapsed:?}"
        );
    }

    #[test]
    fn run_with_timeout_returns_stdout_on_a_fast_success() {
        let mut command = Command::new("echo");
        command.arg("hello");
        let out = run_with_timeout(command, Duration::from_secs(5), "echo").unwrap();
        assert_eq!(out.trim(), "hello");
    }

    // --- git-state fixtures --------------------------------------------------

    /// Run a git subcommand in a test repo with a deterministic identity and no
    /// user/system config bleed-through. Returns the raw output; some setups
    /// (a conflicting merge/cherry-pick/pop) exit non-zero on purpose.
    fn git_run(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap()
    }

    /// [`git_run`] that asserts the subcommand succeeded.
    fn git_ok(dir: &Path, args: &[&str]) {
        let out = git_run(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    /// A repo with one commit of `f.txt` == "base\n" on branch `main`.
    fn init_repo(dir: &Path) {
        git_ok(dir, &["init", "-q"]);
        write(dir, "f.txt", "base\n");
        git_ok(dir, &["add", "."]);
        git_ok(dir, &["commit", "-q", "-m", "base"]);
        git_ok(dir, &["branch", "-m", "main"]);
    }

    /// `main` and `feature` each rewrite `f.txt`'s only line differently, so
    /// merging or cherry-picking one onto the other conflicts. HEAD is `main`.
    fn init_diverged(dir: &Path) {
        init_repo(dir);
        git_ok(dir, &["checkout", "-q", "-b", "feature"]);
        write(dir, "f.txt", "feature\n");
        git_ok(dir, &["commit", "-q", "-am", "feature"]);
        git_ok(dir, &["checkout", "-q", "main"]);
        write(dir, "f.txt", "mainline\n");
        git_ok(dir, &["commit", "-q", "-am", "mainline"]);
    }

    fn repo_state(dir: &Path) -> String {
        status_payload(dir)
            .unwrap()
            .get("repo_state")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn merging_state_maps_and_merge_abort_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        init_diverged(dir.path());
        assert!(!git_run(dir.path(), &["merge", "feature"]).status.success());
        assert_eq!(repo_state(dir.path()), "merging");
        merge_abort(dir.path()).unwrap();
        assert_eq!(repo_state(dir.path()), "clean");
    }

    #[test]
    fn cherry_pick_conflict_maps_and_merge_abort_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        init_diverged(dir.path());
        assert!(!git_run(dir.path(), &["cherry-pick", "feature"])
            .status
            .success());
        assert_eq!(repo_state(dir.path()), "cherry-picking");
        merge_abort(dir.path()).unwrap();
        assert_eq!(repo_state(dir.path()), "clean");
    }

    #[test]
    fn revert_conflict_maps_and_merge_abort_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        write(dir.path(), "f.txt", "second\n");
        git_ok(dir.path(), &["commit", "-q", "-am", "second"]);
        write(dir.path(), "f.txt", "third\n");
        git_ok(dir.path(), &["commit", "-q", "-am", "third"]);
        // Reverting the base->second commit tries to restore "base", but the
        // line is now "third" — a conflict, so the repo enters Revert state.
        assert!(!git_run(dir.path(), &["revert", "--no-edit", "HEAD~1"])
            .status
            .success());
        assert_eq!(repo_state(dir.path()), "reverting");
        merge_abort(dir.path()).unwrap();
        assert_eq!(repo_state(dir.path()), "clean");
    }

    #[test]
    fn bisect_maps_and_merge_abort_resets_it() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let first = String::from_utf8_lossy(&git_run(dir.path(), &["rev-parse", "HEAD"]).stdout)
            .trim()
            .to_string();
        for i in 0..4 {
            write(dir.path(), "f.txt", &format!("v{i}\n"));
            git_ok(dir.path(), &["commit", "-q", "-am", &format!("v{i}")]);
        }
        git_ok(dir.path(), &["bisect", "start"]);
        git_ok(dir.path(), &["bisect", "bad", "HEAD"]);
        git_ok(dir.path(), &["bisect", "good", &first]);
        assert_eq!(repo_state(dir.path()), "bisecting");
        merge_abort(dir.path()).unwrap();
        assert_eq!(repo_state(dir.path()), "clean");
    }

    #[test]
    fn stash_pop_conflict_is_conflicted_not_merging_and_has_no_abort() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        // Stash a change to f.txt, then commit a different change to the same
        // line, so the pop's three-way merge conflicts.
        write(dir.path(), "f.txt", "stashed\n");
        git_ok(dir.path(), &["stash", "push", "-q"]);
        write(dir.path(), "f.txt", "current\n");
        git_ok(dir.path(), &["commit", "-q", "-am", "current"]);
        assert!(!git_run(dir.path(), &["stash", "pop"]).status.success());

        // repo.state() is Clean here (no MERGE_HEAD), but the UU entry makes it
        // "conflicted" — the banner must surface it.
        assert_eq!(repo_state(dir.path()), "conflicted");
        // Nothing git-abortable: merge_abort refuses rather than lying.
        let err = merge_abort(dir.path()).unwrap_err();
        assert!(err.contains("no abortable operation"), "{err}");
    }

    #[test]
    fn checkout_refused_mid_cherry_pick_names_the_cherry_pick() {
        let dir = tempfile::tempdir().unwrap();
        init_diverged(dir.path());
        assert!(!git_run(dir.path(), &["cherry-pick", "feature"])
            .status
            .success());
        let err = checkout(dir.path(), "feature", false).unwrap_err();
        assert!(err.contains("cherry-pick"), "{err}");
        assert!(!err.contains("merge"), "must not name the wrong op: {err}");
    }

    #[test]
    fn checkout_refusal_message_names_each_operation() {
        assert!(checkout_refusal_message("merging").contains("merge"));
        assert!(checkout_refusal_message("rebasing").contains("rebase"));
        assert!(checkout_refusal_message("cherry-picking").contains("cherry-pick"));
        assert!(checkout_refusal_message("reverting").contains("revert"));
        assert!(checkout_refusal_message("bisecting").contains("bisect"));
        // A conflicted tree has no operation to abort — the files are resolved.
        let conflicted = checkout_refusal_message("conflicted");
        assert!(conflicted.contains("resolve"), "{conflicted}");
        assert!(conflicted.contains("conflict"), "{conflicted}");
        // "other" falls back without naming a specific verb.
        assert!(checkout_refusal_message("other").contains("in-progress"));
    }

    #[test]
    fn map_repository_state_covers_every_flavor() {
        use git2::RepositoryState::*;
        assert_eq!(map_repository_state(Clean), "clean");
        assert_eq!(map_repository_state(Merge), "merging");
        assert_eq!(map_repository_state(Rebase), "rebasing");
        assert_eq!(map_repository_state(RebaseInteractive), "rebasing");
        assert_eq!(map_repository_state(RebaseMerge), "rebasing");
        assert_eq!(map_repository_state(CherryPick), "cherry-picking");
        assert_eq!(map_repository_state(CherryPickSequence), "cherry-picking");
        assert_eq!(map_repository_state(Revert), "reverting");
        assert_eq!(map_repository_state(RevertSequence), "reverting");
        assert_eq!(map_repository_state(Bisect), "bisecting");
        assert_eq!(map_repository_state(ApplyMailbox), "other");
        assert_eq!(map_repository_state(ApplyMailboxOrRebase), "other");
    }

    /// A clone of a bare origin that carries `main` and `feature-x`, with no
    /// local `feature-x` — the shape a fresh clone of a team's repository has,
    /// down to the symbolic `origin/HEAD` every clone writes.
    fn clone_of_an_origin_carrying_feature_x(dir: &Path) -> PathBuf {
        let source = dir.join("source");
        std::fs::create_dir(&source).unwrap();
        init_repo(&source);
        git_ok(&source, &["checkout", "-q", "-b", "feature-x"]);
        write(&source, "g.txt", "x\n");
        git_ok(&source, &["add", "."]);
        git_ok(&source, &["commit", "-q", "-m", "feature"]);
        git_ok(&source, &["checkout", "-q", "main"]);
        let origin = dir.join("origin.git");
        git_ok(
            dir,
            &[
                "clone",
                "-q",
                "--bare",
                source.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        let clone = dir.join("clone");
        git_ok(
            dir,
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        clone
    }

    /// A branch the repository holds itself is local, whatever its remotes
    /// also carry: the local ref is the one a checkout would use, and no fetch
    /// can be needed for it.
    #[test]
    fn branch_origin_reads_the_local_ref_before_any_remote() {
        let dir = tempfile::tempdir().unwrap();
        let clone = clone_of_an_origin_carrying_feature_x(dir.path());
        git_ok(
            &clone,
            &["checkout", "-q", "-b", "feature-x", "origin/feature-x"],
        );
        let repo = git2::Repository::open(&clone).unwrap();

        assert_eq!(
            branch_origin(&repo, "feature-x").unwrap(),
            BranchOrigin::Local
        );
        assert_eq!(branch_origin(&repo, "main").unwrap(), BranchOrigin::Local);
    }

    /// A branch only a remote carries names the remote it would be fetched
    /// from and the tracking ref it would be cut at.
    #[test]
    fn branch_origin_finds_a_branch_only_a_remote_carries() {
        let dir = tempfile::tempdir().unwrap();
        let clone = clone_of_an_origin_carrying_feature_x(dir.path());
        let repo = git2::Repository::open(&clone).unwrap();

        assert_eq!(
            branch_origin(&repo, "feature-x").unwrap(),
            BranchOrigin::Remote {
                remote: "origin".to_string(),
                tracking_ref: "refs/remotes/origin/feature-x".to_string(),
            }
        );
    }

    /// Two remotes carrying one branch answer with the one a fetch comes from,
    /// which is the same precedence the listing offers it under — `origin`
    /// first, whatever the other remote sorts as.
    #[test]
    fn branch_origin_prefers_origin_over_another_remote() {
        let dir = tempfile::tempdir().unwrap();
        let clone = clone_of_an_origin_carrying_feature_x(dir.path());
        let origin_path = dir.path().join("origin.git");
        git_ok(
            &clone,
            &["remote", "add", "fork", origin_path.to_str().unwrap()],
        );
        git_ok(&clone, &["fetch", "-q", "fork"]);
        let repo = git2::Repository::open(&clone).unwrap();

        assert_eq!(
            branch_origin(&repo, "feature-x").unwrap(),
            BranchOrigin::Remote {
                remote: "origin".to_string(),
                tracking_ref: "refs/remotes/origin/feature-x".to_string(),
            }
        );
    }

    /// A name no ref of any kind backs is absent — and so is `HEAD`, whose
    /// `refs/remotes/origin/HEAD` is a pointer at a branch, not a branch. A
    /// fetch of it would ask every remote for a ref none of them has.
    #[test]
    fn branch_origin_is_absent_for_an_unknown_name_and_for_a_symbolic_ref() {
        let dir = tempfile::tempdir().unwrap();
        let clone = clone_of_an_origin_carrying_feature_x(dir.path());
        let repo = git2::Repository::open(&clone).unwrap();
        assert!(
            repo.find_reference("refs/remotes/origin/HEAD").is_ok(),
            "the clone has the symbolic ref this test is about"
        );

        assert_eq!(
            branch_origin(&repo, "nobody-cut-this").unwrap(),
            BranchOrigin::Absent
        );
        assert_eq!(branch_origin(&repo, "HEAD").unwrap(), BranchOrigin::Absent);
    }

    /// Only a missing ref means absent. A name git cannot even parse as a ref
    /// is a caller's bug, and it is surfaced rather than answered.
    #[test]
    fn branch_origin_surfaces_an_error_that_is_not_a_missing_ref() {
        let dir = tempfile::tempdir().unwrap();
        let clone = clone_of_an_origin_carrying_feature_x(dir.path());
        let repo = git2::Repository::open(&clone).unwrap();

        assert!(branch_origin(&repo, "not a ref name").is_err());
    }
}
