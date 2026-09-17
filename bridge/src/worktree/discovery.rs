use crate::isolation::{checkout_name, Isolation, WorktreeError};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
/// One git worktree of the project repo that Build did not create (or no longer
/// tracks): the raw material of adoption. Pure data — discovery never mutates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWorktree {
    /// Stable id: "wt-" + the first 12 hex chars of sha256 over the canonical
    /// absolute path (UTF-8 bytes of `path.display().to_string()`).
    pub id: String,
    /// What the checkout is called: [`checkout_name`] of its path, whatever
    /// made it — the name every backend keys its record by, so adoption can
    /// build a `Worktree` that `WorktreeManager::remove` understands. A mount
    /// of a workspace carries its workspace, everything else its directory.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// How this checkout is isolated from the project, read from the checkout.
    pub isolation: Isolation,
    /// Checked-out branch, or None for a detached HEAD (browsable, not adoptable).
    pub branch: Option<String>,
    pub head_sha: String,
    /// HEAD commit subject (`%s`). UNTRUSTED display text.
    pub head_subject: String,
    /// Seconds since the HEAD commit's committer time (clamped at 0).
    pub head_age_seconds: u64,
    /// The HEAD commit's committer time (RFC 3339 UTC). The same fact as
    /// `head_age_seconds` told as an instant rather than a duration, because
    /// the inbox sorts and buckets by instants and an age recomputed every poll
    /// would jitter under the sort. `None` when the stamp cannot be read.
    pub head_committed_at: Option<String>,
    /// `git status --porcelain` line count — staged + unstaged + untracked.
    pub dirty_files: usize,
    /// Commits ahead of [`comparison_ref`](Self::comparison_ref). Retained for
    /// finish-action warnings that describe work the selected ref does not have.
    pub unpushed: Option<u64>,
    /// The configured upstream, or `None` when comparison falls back to the
    /// project's local base branch.
    pub upstream: Option<String>,
    /// The one ref both commit-direction counts are measured against: upstream
    /// when configured, otherwise the project's local base branch.
    pub comparison_ref: Option<String>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Roll-up of `diff_against_merge_base(path, base_branch)` (§2).
    pub diffstat: crate::diff::DiffStat,
    /// The working tree's own uncommitted delta: HEAD vs the index and working
    /// directory, untracked included. What the rail shows as +/− — "what is
    /// sitting here unsaved", which is a different question from how far the
    /// branch has travelled (that is `ahead`/`behind`).
    pub uncommitted: crate::diff::DiffStat,
}

/// The order a checkout list is served in: freshest commit first, canonical
/// path as the tie-break. Shared, because a list amended in place has to stay
/// in the order the scan that filled it used.
pub fn sort_checkouts(checkouts: &mut [ExternalWorktree]) {
    checkouts.sort_by(|a, b| {
        a.head_age_seconds
            .cmp(&b.head_age_seconds)
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// The two facts a branch listing stamps a row with about the project's own
/// repository at `repo_path`: the checkout id it hashes to, and the branch it
/// holds. Read straight off the repository — no status walk, no diffstat, no
/// subprocess — because a listing is the drain's to answer, not a description
/// to render. `None` when the checkout holds no branch: a detached or unborn
/// HEAD, or a bare repository with no working tree. A path git cannot read as a
/// repository is broken rather than branch-less, and says so through the error.
///
/// The branch it holds cannot be checked out a second time, which is the whole
/// reason a listing asks: a workspace is cut from the base ref, never from a
/// branch the repository is standing on.
pub fn repository_branch_holder(
    repo_path: &Path,
) -> Result<Option<(String, String)>, WorktreeError> {
    let canonical_path = std::fs::canonicalize(repo_path)?;
    let repo = git2::Repository::open(&canonical_path)?;
    if repo.is_bare() {
        return Ok(None);
    }
    let head = match repo.head() {
        Ok(head) => head,
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !head.is_branch() {
        return Ok(None);
    }
    Ok(head
        .shorthand()
        .map(|branch| (external_worktree_id(&canonical_path), branch.to_string())))
}

/// Summarize the Build checkout at `path` from the checkout alone, so a clone
/// and a linked worktree are described by one function. `None` when the path is
/// no Build checkout — described by nobody rather than described as something
/// it is not — or when the summary cannot be computed.
pub fn describe_checkout(path: &Path, base_branch: &str, now: i64) -> Option<ExternalWorktree> {
    summarize_checkout(path, Isolation::of(path)?, base_branch, now)
}

/// The summary itself, for a checkout whose isolation the caller already knows.
/// Every fact comes from the checkout: its own repository answers for HEAD, its
/// working tree for what is dirty, and its refs for how far the branch has
/// travelled. A checkout that cannot answer for one of them is skipped, saying
/// so — one broken stray must not fail a scan.
fn summarize_checkout(
    path: &Path,
    isolation: Isolation,
    base_branch: &str,
    now: i64,
) -> Option<ExternalWorktree> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let name = checkout_name(&canonical)?;
    let repo = or_skip(
        "opening the repository",
        &canonical,
        git2::Repository::open(&canonical),
    )?;
    let head = or_skip("HEAD", &canonical, repo.head())?;
    let branch = head
        .is_branch()
        .then(|| head.shorthand().map(str::to_string))
        .flatten();
    let head_oid = head.target()?;
    let commit = or_skip("the HEAD commit", &canonical, repo.find_commit(head_oid))?;
    let dirty_files = or_skip("status", &canonical, worktree_status_line_count(&canonical))?;
    // Counts only — the board never shows this tree's patch, so never render one.
    let diffstat = or_skip(
        "the diff against the merge base",
        &canonical,
        crate::diff::stat_against_merge_base(&canonical, base_branch),
    )?;
    // What is sitting in this tree unsaved — the +/− the rail shows. Distinct
    // from the diffstat above, which is everything the branch carries.
    let uncommitted = or_skip(
        "the uncommitted diff",
        &canonical,
        crate::diff::stat_uncommitted(&canonical),
    )?;
    let comparison = branch_comparison(&repo, &commit, branch.as_deref(), base_branch);

    Some(ExternalWorktree {
        id: external_worktree_id(&canonical),
        name,
        path: canonical,
        isolation,
        branch,
        head_sha: head_oid.to_string(),
        head_subject: commit.summary().unwrap_or("").to_string(),
        head_age_seconds: (now - commit.time().seconds()).max(0) as u64,
        head_committed_at: rfc3339_from_unix(commit.time().seconds()),
        dirty_files,
        unpushed: comparison.ahead,
        upstream: comparison.upstream,
        comparison_ref: comparison.reference,
        ahead: comparison.ahead,
        behind: comparison.behind,
        diffstat,
        uncommitted,
    })
}

use super::command::unix_now;
use super::comparison::{branch_comparison, or_skip, worktree_status_line_count};
use super::identity::{canonical_root, external_worktree_id, rfc3339_from_unix};
use super::WorktreeManager;

impl WorktreeManager {
    /// Every checkout of this project that is neither the project's own nor in
    /// `excluded` (Build-bound checkouts, which must never surface as
    /// adoptable, in whatever spelling the caller holds them: they are
    /// canonicalized here, beside the repository path, so the decide phase that
    /// collects them makes no filesystem call), with a review summary each. Read-only apart
    /// from the base sync each checkout needs before its counts mean anything.
    /// A checkout whose summary cannot be computed is skipped — one broken
    /// stray must not fail the scan.
    pub fn discover(
        &self,
        base_branch: &str,
        excluded: &HashSet<PathBuf>,
    ) -> Result<Vec<ExternalWorktree>, WorktreeError> {
        let excluded: HashSet<PathBuf> = excluded.iter().map(|path| canonical_root(path)).collect();
        let mut paths: Vec<PathBuf> = Vec::new();
        for backend in self.every_backend() {
            for path in backend.discover(&self.repo_path, &self.worktrees_root)? {
                if !excluded.contains(&path) && !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
        let now = unix_now();
        let mut found: Vec<ExternalWorktree> = Vec::new();
        for path in paths {
            if let Err(error) = self.sync_base(&path, base_branch) {
                eprintln!("discover: base sync failed for {}: {error}", path.display());
            }
            if let Some(summary) = describe_checkout(&path, base_branch, now) {
                found.push(summary);
            }
        }
        found.sort_by(|a, b| {
            a.head_age_seconds
                .cmp(&b.head_age_seconds)
                .then_with(|| a.path.cmp(&b.path))
        });
        Ok(found)
    }
}
