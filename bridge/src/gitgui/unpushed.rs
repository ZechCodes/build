use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkSummary {
    pub pushes: u64,
    /// Commits in the configured push destination that HEAD cannot reach.
    /// Zero when there is no concrete push destination to compare.
    pub behind: u64,
    pub additions: u64,
    pub deletions: u64,
    /// The working tree holds something Git has not been told to keep: a
    /// staged or unstaged edit, an untracked file, or an operation left half
    /// done. Deliberately independent of line counts — an empty untracked file
    /// or a binary edit carries a +0/-0 stat.
    pub dirty: bool,
    /// No local commit, index, worktree, untracked file, or in-progress Git
    /// operation remains: `pushes == 0 && !dirty`.
    pub clean: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PublishedBase {
    PushTarget {
        oid: Option<git2::Oid>,
        target: git2::Oid,
        label: String,
    },
    PublishedAncestor(git2::Oid),
    Empty,
}

impl PublishedBase {
    fn oid(&self) -> Option<git2::Oid> {
        match self {
            Self::PushTarget { oid, .. } => *oid,
            Self::PublishedAncestor(oid) => Some(*oid),
            Self::Empty => None,
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::PushTarget { label, .. } => json!({ "kind": "push_target", "label": label }),
            Self::PublishedAncestor(_) => json!({ "kind": "published_ancestor", "label": null }),
            Self::Empty => json!({ "kind": "empty", "label": null }),
        }
    }

    fn push_target_exists(&self) -> bool {
        matches!(self, Self::PushTarget { .. })
    }

    fn behind(&self, repo: &git2::Repository, head: git2::Oid) -> Result<u64, String> {
        let Self::PushTarget { target, .. } = self else {
            return Ok(0);
        };
        repo.graph_ahead_behind(head, *target)
            .map_err(|error| error.to_string())?
            .1
            .try_into()
            .map_err(|_| "workspace behind count exceeds u64".to_string())
    }
}

fn direct_remote_tips(repo: &git2::Repository) -> Result<Vec<git2::Oid>, String> {
    let mut tips = Vec::new();
    let refs = repo
        .references_glob("refs/remotes/*/*")
        .map_err(|e| e.to_string())?;
    for reference in refs {
        let reference = reference.map_err(|e| e.to_string())?;
        if reference.kind() == Some(git2::ReferenceType::Direct) {
            if let Some(oid) = reference.target() {
                tips.push(oid);
            }
        }
    }
    tips.sort_unstable();
    tips.dedup();
    Ok(tips)
}

fn nearest_published_ancestor(
    repo: &git2::Repository,
    head: git2::Oid,
    tips: &[git2::Oid],
) -> Result<Option<git2::Oid>, String> {
    if tips.is_empty() {
        return Ok(None);
    }
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL)
        .map_err(|e| e.to_string())?;
    walk.push(head).map_err(|e| e.to_string())?;
    for tip in tips {
        walk.hide(*tip).map_err(|e| e.to_string())?;
    }
    let unpublished = walk
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if unpublished.is_empty() {
        return Ok(Some(head));
    }
    let unpublished_set = unpublished
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for oid in unpublished {
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        for parent in commit.parent_ids() {
            if !unpublished_set.contains(&parent) && repo.find_commit(parent).is_ok() {
                return Ok(Some(parent));
            }
        }
    }
    Ok(None)
}

fn published_base(repo: &git2::Repository) -> Result<PublishedBase, String> {
    let head = match repo.head().ok().and_then(|head| head.target()) {
        Some(head) => head,
        None => return Ok(PublishedBase::Empty),
    };
    if let Ok(branch) = repo.head().and_then(|head| {
        if !head.is_branch() {
            return Err(git2::Error::from_str("detached"));
        }
        head.shorthand()
            .map(str::to_string)
            .ok_or_else(|| git2::Error::from_str("unnamed branch"))
    }) {
        if let Some(remote) = push_remote(repo, &branch) {
            let label = format!("{remote}/{branch}");
            if let Ok(reference) = repo.find_reference(&format!("refs/remotes/{label}")) {
                if let Some(target) = reference.target() {
                    let base = repo.merge_base(head, target).ok();
                    return Ok(PublishedBase::PushTarget {
                        oid: base,
                        target,
                        label,
                    });
                }
            }
        }
    }
    let tips = direct_remote_tips(repo)?;
    Ok(match nearest_published_ancestor(repo, head, &tips)? {
        Some(oid) => PublishedBase::PublishedAncestor(oid),
        None => PublishedBase::Empty,
    })
}

fn commit_ids_since(
    repo: &git2::Repository,
    base: Option<git2::Oid>,
) -> Result<HashSet<git2::Oid>, String> {
    let Some(head) = repo.head().ok().and_then(|head| head.target()) else {
        return Ok(HashSet::new());
    };
    let mut walk = repo.revwalk().map_err(|error| error.to_string())?;
    walk.push(head).map_err(|error| error.to_string())?;
    if let Some(base) = base {
        walk.hide(base).map_err(|error| error.to_string())?;
    }
    Ok(walk
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .collect())
}

/// Commits represented by `git.unpushed`, plus a key naming the exact base
/// used to classify them. History uses this rather than independently guessing
/// from upstream/ahead counts, so its highlights cannot drift from All changes.
pub(super) fn unpublished_commits(
    repo: &git2::Repository,
) -> Result<(HashSet<git2::Oid>, String), String> {
    let base = published_base(repo)?;
    let commits = commit_ids_since(repo, base.oid())?;
    let key = crate::diff::fnv1a64_hex(&format!("unpushed\0{base:?}"));
    Ok((commits, key))
}

/// Counts the work not represented by this checkout's publication base.
/// The diff is base-to-worktree, so committed and dirty edits are represented
/// once in their final form rather than by adding per-commit diffstats.
pub fn work_summary(repo_path: &Path) -> Result<WorkSummary, String> {
    let repo = git2::Repository::open_ext(
        repo_path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .map_err(|error| error.to_string())?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "repository has no working directory".to_string())?;
    let requested = repo_path
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let opened = workdir.canonicalize().map_err(|error| error.to_string())?;
    if opened != requested {
        return Err("path is not the repository working directory".to_string());
    }
    let base = published_base(&repo)?;
    let head = repo.head().ok().and_then(|head| head.target());
    let pushes = commit_ids_since(&repo, base.oid())?
        .len()
        .try_into()
        .map_err(|_| "unpushed commit count exceeds u64".to_string())?;
    let behind = match head {
        Some(head) => base.behind(&repo, head)?,
        None => 0,
    };
    let mut status_options = git2::StatusOptions::new();
    status_options
        .include_untracked(true)
        .recurse_untracked_dirs(true);
    let dirty = repo.state() != git2::RepositoryState::Clean
        || !repo
            .statuses(Some(&mut status_options))
            .map_err(|error| error.to_string())?
            .is_empty();
    let stat = crate::diff::diff_against_commit(repo_path, base.oid())
        .map_err(|error| error.to_string())?
        .stat();
    Ok(WorkSummary {
        pushes,
        behind,
        additions: stat.insertions as u64,
        deletions: stat.deletions as u64,
        dirty,
        clean: pushes == 0 && !dirty,
    })
}

pub fn aggregate_work_summary(repo_paths: &[std::path::PathBuf]) -> Result<WorkSummary, String> {
    repo_paths.iter().try_fold(
        WorkSummary {
            pushes: 0,
            behind: 0,
            additions: 0,
            deletions: 0,
            dirty: false,
            clean: true,
        },
        |total, path| {
            let summary = work_summary(path)?;
            Ok(WorkSummary {
                pushes: total
                    .pushes
                    .checked_add(summary.pushes)
                    .ok_or_else(|| "workspace push count exceeds u64".to_string())?,
                behind: total
                    .behind
                    .checked_add(summary.behind)
                    .ok_or_else(|| "workspace behind count exceeds u64".to_string())?,
                additions: total
                    .additions
                    .checked_add(summary.additions)
                    .ok_or_else(|| "workspace addition count exceeds u64".to_string())?,
                deletions: total
                    .deletions
                    .checked_add(summary.deletions)
                    .ok_or_else(|| "workspace deletion count exceeds u64".to_string())?,
                dirty: total.dirty || summary.dirty,
                clean: total.clean && summary.clean,
            })
        },
    )
}

/// The same destination precedence used by workspace finish. Reading config
/// and remote refs is local and cannot mutate or contact the repository.
fn push_remote(repo: &git2::Repository, branch: &str) -> Option<String> {
    let config = repo.config().ok()?;
    config
        .get_string(&format!("branch.{branch}.pushRemote"))
        .ok()
        .or_else(|| config.get_string("remote.pushDefault").ok())
        .or_else(|| config.get_string(&format!("branch.{branch}.remote")).ok())
        .filter(|remote| remote != "." && !remote.trim().is_empty())
        .or_else(|| {
            repo.find_remote("origin")
                .ok()
                .map(|_| "origin".to_string())
        })
}

fn file_rows(diff: &crate::diff::WorktreeDiff) -> Vec<Value> {
    diff.files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect()
}

fn file_edited_at(
    repo_path: &Path,
    diff: &crate::diff::WorktreeDiff,
) -> serde_json::Map<String, Value> {
    diff.files()
        .iter()
        .filter_map(|file| {
            crate::diff::file_edited_at(repo_path, &file.path)
                .map(|at| (file.path.clone(), json!(at)))
        })
        .collect()
}

/// The key naming the whole delta between a checkout and its publication
/// base — the base itself and the tree above it, so a client holding it holds
/// this exact answer.
fn unpushed_diff_key(repo_path: &Path, base: &PublishedBase) -> Result<String, String> {
    let delta_key =
        crate::diff::key_against_commit(repo_path, base.oid()).map_err(|e| e.to_string())?;
    Ok(crate::diff::fnv1a64_hex(&format!(
        "{:?}\0{}",
        base, delta_key
    )))
}

/// The commits above the publication base, newest first — what a checkout
/// holds that its push destination does not, capped at
/// [`UNPUSHED_COMMITS_MAX`].
///
/// The cap is the point: a checkout with no base at all — a repository with
/// no remote — has its entire history standing above that base, and this
/// list rides a push. The walk streams, so stopping at the cap is the
/// cheaper walk as well as the smaller item.
fn unpublished_commit_list(
    repo: &git2::Repository,
    base: Option<git2::Oid>,
) -> Result<Vec<Value>, String> {
    let Some(head) = repo.head().ok().and_then(|head| head.target()) else {
        return Ok(Vec::new());
    };
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    walk.push(head).map_err(|e| e.to_string())?;
    if let Some(base) = base {
        walk.hide(base).map_err(|e| e.to_string())?;
    }
    let mut commits = Vec::new();
    for oid in walk.take(crate::changes::UNPUSHED_COMMITS_MAX) {
        let commit = repo
            .find_commit(oid.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        commits.push(super::history::commit_summary_json(&commit));
    }
    Ok(commits)
}

/// What a `changes` push says about unpublished work: the base it is measured
/// against, the commits standing above that base, and the key naming the diff
/// between them. No patch — the bytes are what a reviewer opening All changes
/// asks [`unpushed_payload`] for.
pub fn unpushed_summary(repo_path: &Path) -> Result<Value, String> {
    let repo = git2::Repository::open(repo_path).map_err(|e| e.to_string())?;
    let base = published_base(&repo)?;
    Ok(json!({
        "base": base.json(),
        "published": base.push_target_exists(),
        "commits": unpublished_commit_list(&repo, base.oid())?,
        "diff_key": unpushed_diff_key(repo_path, &base)?,
    }))
}

pub fn unpushed_payload(
    repo_path: &Path,
    if_diff_key: Option<&str>,
    with_patch: bool,
) -> Result<Value, String> {
    let repo = git2::Repository::open(repo_path).map_err(|e| e.to_string())?;
    let base = published_base(&repo)?;
    let key = unpushed_diff_key(repo_path, &base)?;
    if if_diff_key == Some(&key) {
        return Ok(json!({ "unchanged": true, "diff_key": key }));
    }
    let diff =
        crate::diff::diff_against_commit(repo_path, base.oid()).map_err(|e| e.to_string())?;
    let files = file_rows(&diff);
    let mut payload = json!({
        "stat": diff.stat().to_json(),
        "files": files,
        "file_edited_at": file_edited_at(repo_path, &diff),
        "diff_key": key,
        "published": base.push_target_exists(),
        "base": base.json(),
    });
    if with_patch {
        payload["patch"] = json!(diff.patch());
    }
    Ok(payload)
}
