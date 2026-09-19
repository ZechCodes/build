use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

/// Which commit rows a caller needs classified. Run/worktree histories retain
/// their base-branch meaning; workspace histories use the publication base
/// shared with `git.unpushed`.
#[derive(Debug, Clone, Copy)]
pub enum LogHighlight<'a> {
    AheadOfBase(&'a str),
    Unpushed,
}

struct CommitMarker {
    field: &'static str,
    commits: HashSet<git2::Oid>,
    key: String,
}

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

pub(super) fn open_repo(repo_path: &Path) -> Result<git2::Repository, String> {
    git2::Repository::open(repo_path).map_err(|e| format!("cannot open repository: {e}"))
}

/// The identity Git can recover from HEAD itself.
///
/// A detached HEAD deliberately carries only a commit id. Git does not retain
/// which tag (if any) was used to reach that commit, and several tags may point
/// at the same commit, so callers must not guess a selected tag from equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HeadIdentity {
    Branch { name: String, full_ref: String },
    Detached { commit: git2::Oid },
}

/// Whether a `repo.head()` error means "no commits yet" rather than a broken repo.
fn is_unborn_head_error(error: &git2::Error) -> bool {
    matches!(
        error.code(),
        git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
    )
}

/// Read HEAD without collapsing a detached checkout into a pretend branch.
/// An unborn symbolic HEAD is still a branch identity even though its local
/// ref has not been created yet.
pub(super) fn head_identity(repo: &git2::Repository) -> Result<HeadIdentity, String> {
    match repo.head() {
        Ok(head) if head.is_branch() => {
            let full_ref = head
                .name()
                .ok_or_else(|| "cannot read HEAD branch name".to_string())?
                .to_string();
            let name = full_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&full_ref)
                .to_string();
            Ok(HeadIdentity::Branch { name, full_ref })
        }
        Ok(head) => {
            let commit = head
                .peel_to_commit()
                .map_err(|e| format!("cannot resolve HEAD: {e}"))?
                .id();
            Ok(HeadIdentity::Detached { commit })
        }
        Err(error) if is_unborn_head_error(&error) => {
            let head_ref = repo
                .find_reference("HEAD")
                .map_err(|e| format!("cannot read HEAD: {e}"))?;
            let full_ref = head_ref
                .symbolic_target()
                .ok_or_else(|| "cannot read unborn HEAD target".to_string())?
                .to_string();
            let name = full_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&full_ref)
                .to_string();
            Ok(HeadIdentity::Branch { name, full_ref })
        }
        Err(error) => Err(format!("cannot read HEAD: {error}")),
    }
}

/// The checked-out branch name: HEAD's shorthand, or — on an unborn HEAD —
/// the shorthand of the branch HEAD symbolically points at.
pub(super) fn current_branch(repo: &git2::Repository) -> Result<String, String> {
    match head_identity(repo)? {
        HeadIdentity::Branch { name, .. } => Ok(name),
        HeadIdentity::Detached { .. } => Ok("HEAD".to_string()),
    }
}

/// HEAD's commit id, or `None` when HEAD is unborn.
pub(super) fn head_commit_id(repo: &git2::Repository) -> Result<Option<git2::Oid>, String> {
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
pub(super) fn commit_summary_json(commit: &git2::Commit) -> Value {
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
) -> Result<(HashSet<git2::Oid>, String), String> {
    let base_tip = repo
        .revparse_single(base_branch)
        .map_err(|e| format!("cannot resolve base branch {base_branch}: {e}"))?
        .peel_to_commit()
        .map_err(|e| format!("base branch {base_branch} is not a commit: {e}"))?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    walk.hide(base_tip.id()).map_err(|e| e.to_string())?;
    let commits = walk
        .map(|oid| oid.map_err(|e| e.to_string()))
        .collect::<Result<HashSet<_>, _>>()?;
    let key = crate::diff::fnv1a64_hex(&format!("ahead_of_base\0{}", base_tip.id()));
    Ok((commits, key))
}

/// Where a cursored log starts, once the cursor has been read against the
/// history it claims to sit in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogCursor {
    /// Nobody named one: the page is the latest, as it always was.
    Unnamed,
    /// The cursor is behind HEAD, so the page is what landed after it.
    Behind(git2::Oid),
    /// HEAD cannot reach the cursor — a rebase, a reset, a hash from another
    /// checkout, a commit this repository has never held. The page is the
    /// latest one again and the client is told to replace rather than
    /// prepend.
    Reset,
}

/// Read a `since` cursor against the history HEAD reaches.
///
/// An unreadable cursor is a refusal (it is a param, validated exactly as
/// `git.show`'s hash is); a readable one HEAD cannot reach is not — the
/// client's log is simply stale in a way prepending cannot fix.
fn log_cursor(
    repo: &git2::Repository,
    since: Option<&str>,
    head: git2::Oid,
) -> Result<LogCursor, String> {
    let Some(since) = since else {
        return Ok(LogCursor::Unnamed);
    };
    if !is_valid_hash_prefix(since) {
        return Err("since must be 4-40 lowercase hex characters".to_string());
    }
    let Ok(commit) = resolve_commit_prefix(repo, since) else {
        return Ok(LogCursor::Reset);
    };
    // A commit is not its own descendant, and a client whose cursor IS HEAD
    // is the caught-up case this read exists to answer cheaply.
    if commit.id() == head {
        return Ok(LogCursor::Behind(head));
    }
    match repo.graph_descendant_of(head, commit.id()) {
        Ok(true) => Ok(LogCursor::Behind(commit.id())),
        Ok(false) => Ok(LogCursor::Reset),
        Err(error) => Err(format!("cannot read history: {error}")),
    }
}

/// Which commits a page marks, and under what key — the highlight mode's
/// whole effect on a log.
fn commit_marker(
    repo: &git2::Repository,
    highlight: Option<LogHighlight<'_>>,
) -> Result<Option<CommitMarker>, String> {
    let (field, (commits, key)) = match highlight {
        Some(LogHighlight::AheadOfBase(base_branch)) => {
            ("ahead_of_base", commits_ahead_of(repo, base_branch)?)
        }
        Some(LogHighlight::Unpushed) => ("unpushed", super::unpushed::unpublished_commits(repo)?),
        None => return Ok(None),
    };
    Ok(Some(CommitMarker {
        field,
        commits,
        key,
    }))
}

/// One page of commit history from HEAD, topological newest-first. With
/// a highlight mode each entry carries the matching boolean and the result
/// carries a key for that classification. Project scope omits both entirely.
///
/// `since` makes the read cursored: the page is then `since..HEAD` rather
/// than the latest commits, and `newest` is the cursor the client stores for
/// next time. Both extra fields ride every answer, cursored or not, so a
/// client can start holding a cursor from any page it has.
pub fn log_page(
    repo_path: &Path,
    highlight: Option<LogHighlight<'_>>,
    limit: usize,
    skip: usize,
    since: Option<&str>,
) -> Result<Value, String> {
    let repo = open_repo(repo_path)?;
    let branch = current_branch(&repo)?;
    let Some(head) = head_commit_id(&repo)? else {
        // Nothing committed yet: no cursor to hand back, and nothing a stale
        // one could be stale against.
        return Ok(json!({
            "branch": branch, "commits": [], "more": false, "reset": false,
        }));
    };
    let cursor = log_cursor(&repo, since, head)?;
    let marker = commit_marker(&repo, highlight)?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    walk.push_head().map_err(|e| e.to_string())?;
    if let LogCursor::Behind(oid) = cursor {
        walk.hide(oid).map_err(|e| e.to_string())?;
    }
    let mut page = walk.skip(skip);
    let mut commits = Vec::with_capacity(limit);
    for _ in 0..limit {
        let Some(oid) = page.next().transpose().map_err(|e| e.to_string())? else {
            break;
        };
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let mut entry = commit_summary_json(&commit);
        if let Some(marker) = &marker {
            entry[marker.field] = json!(marker.commits.contains(&oid));
        }
        commits.push(entry);
    }
    let more = page
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .is_some();
    let mut payload = json!({
        "branch": branch,
        "commits": commits,
        "more": more,
        "reset": cursor == LogCursor::Reset,
        "newest": head.to_string(),
    });
    if let Some(marker) = marker {
        payload["highlight_key"] = json!(marker.key);
    }
    Ok(payload)
}

/// Whether `hash` is an acceptable `git.show` argument: 4–40 lowercase hex
/// characters — an object-id prefix, never a general revspec.
pub(super) fn is_valid_hash_prefix(hash: &str) -> bool {
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

/// One commit's diff, as `git.show` may answer with it: the exact counts,
/// the whole patch, and the file headers on their own.
///
/// The headers are what a capped answer carries instead of the patch — which
/// files moved, and how git names each move — so a client that asked for a
/// small answer gets a whole small thing rather than the top of a large one.
struct CommitPatch {
    stat: Value,
    patch: String,
    headers: String,
}

/// Unified patch + exact counts for `old_tree` → `new_tree` (`None` = empty
/// tree, for root commits). Counting happens during the print walk, same as
/// `diff.rs`, so the numbers always match the (pre-truncation) patch.
fn tree_diff_patch(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
    new_tree: &git2::Tree,
) -> Result<CommitPatch, String> {
    let mut options = crate::diff::canonical_patch_options();
    let diff = repo
        .diff_tree_to_tree(old_tree, Some(new_tree), Some(&mut options))
        .map_err(|e| e.to_string())?;
    let files_changed = diff.deltas().len();
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut patch = String::new();
    let mut headers = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            '+' => insertions += 1,
            '-' => deletions += 1,
            // One callback per file carries that file's whole header.
            'F' => headers.push_str(&String::from_utf8_lossy(line.content())),
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
    Ok(CommitPatch {
        stat,
        patch,
        headers,
    })
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

/// How much of a commit's diff one answer carries, and whether that is all
/// of it.
///
/// A caller that named `max_bytes` is caching rather than rendering: past its
/// cap it gets the file headers, which say which files moved without saying
/// how, and asks again when a reviewer opens the commit. A caller that named
/// nothing gets the patch cut at the wire cap, as it always did.
fn capped_patch(patch: String, headers: String, max_bytes: Option<usize>) -> (String, bool) {
    match max_bytes {
        Some(cap) if patch.len() > cap => (truncate_at_utf8_boundary(headers, cap).0, true),
        _ => truncate_at_utf8_boundary(patch, GIT_SHOW_MAX_PATCH_BYTES),
    }
}

/// `git.show`: one commit's metadata, exact stat, and (capped) patch against
/// its first parent — the empty tree for a root commit.
///
/// `max_bytes` is the caller's own cap, already clamped; `patch_bytes` names
/// the whole patch's size whether or not it all shipped.
pub fn show_commit(
    repo_path: &Path,
    hash: &str,
    max_bytes: Option<usize>,
) -> Result<Value, String> {
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
    let CommitPatch {
        stat,
        patch,
        headers,
    } = tree_diff_patch(&repo, parent_tree.as_ref(), &commit_tree)?;
    let mut result = commit_summary_json(&commit);
    let (body, _) =
        truncate_at_utf8_boundary(commit.body().unwrap_or("").to_string(), GIT_BODY_MAX_BYTES);
    result["body"] = json!(body);
    result["stat"] = stat;
    result["patch_bytes"] = json!(patch.len());
    let (patch, truncated) = capped_patch(patch, headers, max_bytes);
    result["patch"] = json!(patch);
    result["truncated"] = json!(truncated);
    Ok(result)
}
