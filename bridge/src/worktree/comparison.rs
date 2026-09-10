use crate::git_process::run_git;
use crate::isolation::WorktreeError;
use std::path::Path;
/// A fact a checkout summary needs, or nothing — and the one place a skipped
/// checkout says why it was skipped.
pub(super) fn or_skip<T, E: std::fmt::Display>(
    what: &str,
    path: &Path,
    fact: Result<T, E>,
) -> Option<T> {
    fact.inspect_err(|error| {
        eprintln!("summarize_checkout: {what} for {}: {error}", path.display());
    })
    .ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BranchComparison {
    pub reference: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
}

/// Compare HEAD in both directions with its configured upstream, falling back
/// to the local base branch only when there is no upstream.
pub(crate) fn branch_comparison(
    repo: &git2::Repository,
    head: &git2::Commit,
    branch: Option<&str>,
    base_branch: &str,
) -> BranchComparison {
    if let Some((name, oid)) = branch.and_then(|b| upstream_of(repo, b)) {
        let counts = repo.graph_ahead_behind(head.id(), oid).ok();
        return BranchComparison {
            reference: Some(name.clone()),
            upstream: Some(name),
            ahead: counts.map(|(ahead, _)| ahead as u64),
            behind: counts.map(|(_, behind)| behind as u64),
        };
    }
    let counts = resolve_commit(repo, base_branch)
        .and_then(|base| repo.graph_ahead_behind(head.id(), base.id()).ok());
    BranchComparison {
        reference: counts.map(|_| base_branch.to_string()),
        upstream: None,
        ahead: counts.map(|(ahead, _)| ahead as u64),
        behind: counts.map(|(_, behind)| behind as u64),
    }
}

/// The commit a revspec names, or `None` when it does not resolve.
fn resolve_commit<'repo>(
    repo: &'repo git2::Repository,
    revspec: &str,
) -> Option<git2::Commit<'repo>> {
    repo.revparse_single(revspec)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
}

/// A local branch's upstream, as (ref shorthand, tip) — `None` when the branch
/// tracks nothing, or its upstream ref is gone.
fn upstream_of(repo: &git2::Repository, branch: &str) -> Option<(String, git2::Oid)> {
    let upstream = repo
        .find_branch(branch, git2::BranchType::Local)
        .ok()?
        .upstream()
        .ok()?;
    let name = upstream.name().ok().flatten()?.to_string();
    let oid = upstream.get().target()?;
    Some((name, oid))
}

/// Count of non-empty `git status --porcelain` lines in `worktree_path`.
pub(super) fn worktree_status_line_count(worktree_path: &Path) -> Result<usize, WorktreeError> {
    Ok(run_git(worktree_path, &["status", "--porcelain"])?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}
