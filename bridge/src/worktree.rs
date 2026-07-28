//! Git worktree lifecycle: one worktree + branch per task.
//!
//! The bridge owns worktrees. A task gets an isolated branch (`build/<slug>`) and
//! a working directory cut from the project's base branch, so parallel tasks on
//! the same repo never touch each other. On abandon the worktree is removed but
//! the branch is kept (abandoning stays reversible-ish); merge decides for itself.
//! Disposable planning worktrees (Plan/Run split) use the same manager with the
//! `plan/<slug>` branch namespace instead.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The branch-name prefix for every run/task branch: `build/<slug>`.
pub const BRANCH_PREFIX: &str = "build";

/// The branch-name prefix for disposable planning worktrees: `plan/<slug>`.
pub const PLAN_BRANCH_PREFIX: &str = "plan";

/// Things that can go wrong managing a worktree.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Command(String),
}

/// A task's worktree: where it lives, which branch it's on, and what it was cut
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Git's internal worktree id (also the on-disk directory name) — the slug.
    pub name: String,
    /// Absolute path to the working directory.
    pub path: PathBuf,
    /// The task branch, e.g. `build/fix-the-typo`.
    pub branch: String,
    /// The branch this worktree was created from.
    pub base_branch: String,
}

/// Derive a filesystem- and branch-safe slug from a free-text goal.
///
/// Lowercases, collapses any run of non-alphanumerics to a single hyphen, trims
/// hyphens, truncates, and falls back to `task` if nothing survives.
pub fn slugify(goal: &str) -> String {
    let mut slug = String::new();
    let mut prev_hyphen = false;
    for ch in goal.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen && !slug.is_empty() {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug: String = slug.chars().take(50).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

/// Owns worktree creation and teardown for a single project repository.
pub struct WorktreeManager {
    repo_path: PathBuf,
    worktrees_root: PathBuf,
    /// Branch namespace for created worktrees (`<prefix>/<slug>`): `build` for
    /// run worktrees, `plan` for disposable planning worktrees. Removal never
    /// re-derives the branch (the [`Worktree`] carries it), so one manager can
    /// tear down another prefix's worktree.
    branch_prefix: String,
}

impl WorktreeManager {
    /// `repo_path` is the project git repo; `worktrees_root` is where task
    /// worktrees are materialized (one subdirectory per task slug). Branches
    /// are cut in the `build/` namespace unless overridden with
    /// [`with_branch_prefix`](Self::with_branch_prefix).
    pub fn new(repo_path: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        WorktreeManager {
            repo_path: repo_path.into(),
            worktrees_root: worktrees_root.into(),
            branch_prefix: BRANCH_PREFIX.to_string(),
        }
    }

    /// Cut branches in a different namespace (`<prefix>/<slug>`). Planning
    /// worktrees use `plan/` so they never collide with run branches of the
    /// same slug.
    pub fn with_branch_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.branch_prefix = prefix.into();
        self
    }

    /// Create `<prefix>/<slug>` from `base_branch` and add a worktree for it. The
    /// name is made unique (`<slug>`, `<slug>-2`, …) so re-dispatching the same
    /// goal — or leftover branches/worktrees from prior tasks — never collides.
    pub fn create(&self, slug: &str, base_branch: &str) -> Result<Worktree, WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;
        let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
        std::fs::create_dir_all(&self.worktrees_root)?;

        let mut name = slug.to_string();
        let mut n = 2;
        while self.name_taken(&repo, &name) {
            name = format!("{slug}-{n}");
            n += 1;
        }
        let branch = self.branch_name(&name);

        // Cut the task branch from the tip of the base branch.
        repo.branch(&branch, &base_commit, false)?;
        let path = self.worktrees_root.join(&name);

        // Point the worktree at the branch we just created.
        let branch_ref = repo.find_reference(&format!("refs/heads/{branch}"))?;
        let mut opts = git2::WorktreeAddOptions::new();
        opts.reference(Some(&branch_ref));
        repo.worktree(&name, &path, Some(&opts))?;

        Ok(Worktree {
            name,
            path,
            branch,
            base_branch: base_branch.to_string(),
        })
    }

    /// Whether a candidate name is already in use as a branch, a registered
    /// worktree, or an on-disk directory.
    fn name_taken(&self, repo: &git2::Repository, name: &str) -> bool {
        repo.find_branch(&self.branch_name(name), git2::BranchType::Local)
            .is_ok()
            || repo.find_worktree(name).is_ok()
            || self.worktrees_root.join(name).exists()
    }

    /// Build the branch name for a slug in this manager's namespace.
    fn branch_name(&self, slug: &str) -> String {
        format!("{}/{}", self.branch_prefix, slug)
    }

    /// Remove the worktree's working directory and prune git's record of it. When
    /// `keep_branch` is false the task branch is deleted too.
    pub fn remove(&self, worktree: &Worktree, keep_branch: bool) -> Result<(), WorktreeError> {
        let repo = git2::Repository::open(&self.repo_path)?;

        // Drop the working directory, then prune git's bookkeeping for it.
        if worktree.path.exists() {
            std::fs::remove_dir_all(&worktree.path)?;
        }
        let gwt = repo.find_worktree(&worktree.name)?;
        let mut prune = git2::WorktreePruneOptions::new();
        prune.valid(true).working_tree(true);
        gwt.prune(Some(&mut prune))?;

        // The branch is only deletable once it is no longer checked out.
        if !keep_branch {
            repo.find_branch(&worktree.branch, git2::BranchType::Local)?
                .delete()?;
        }
        Ok(())
    }
}

/// One git worktree of the project repo that Build did not create (or no longer
/// tracks): the raw material of adoption. Pure data — discovery never mutates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalWorktree {
    /// Stable id: "wt-" + the first 12 hex chars of sha256 over the canonical
    /// absolute path (UTF-8 bytes of `path.display().to_string()`).
    pub id: String,
    /// Git's internal worktree name (`repo.find_worktree(name)` works) — kept so
    /// adoption can build a `Worktree` that `WorktreeManager::remove` understands.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// Checked-out branch, or None for a detached HEAD (browsable, not adoptable).
    pub branch: Option<String>,
    pub head_sha: String,
    /// HEAD commit subject (`%s`). UNTRUSTED display text.
    pub head_subject: String,
    /// Seconds since the HEAD commit's committer time (clamped at 0).
    pub head_age_seconds: u64,
    /// `git status --porcelain` line count — staged + unstaged + untracked.
    pub dirty_files: usize,
    /// Commits this worktree has that [`sync_base`](Self::sync_base) does not,
    /// and vice versa. `None` when nothing could be compared against, which is a
    /// different thing from being level with it.
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// What `ahead`/`behind` were measured against: the branch's upstream when
    /// it tracks one — "have I pushed this?" is the question a tracking branch
    /// asks — otherwise the project's base branch, which is the only thing a
    /// branch with nowhere to push can be measured by.
    pub sync_base: Option<String>,
    /// Roll-up of `diff_against_merge_base(path, base_branch)` (§2).
    pub diffstat: crate::diff::DiffStat,
    /// The working tree's own uncommitted delta: HEAD vs the index and working
    /// directory, untracked included. What the rail shows as +/− — "what is
    /// sitting here unsaved", which is a different question from how far the
    /// branch has travelled (that is `ahead`/`behind`).
    pub uncommitted: crate::diff::DiffStat,
}

/// The stable external-worktree id for a canonical absolute path.
pub fn external_worktree_id(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("wt-{}", &hex[..12])
}

/// Branch stems that carry no meaningful goal on their own — adoption falls
/// back to the HEAD commit subject for these.
const GENERIC_BRANCH_STEMS: &[&str] = &[
    "main", "master", "dev", "develop", "wip", "tmp", "temp", "test", "testing", "scratch",
    "patch", "fix", "feature", "new", "branch",
];

/// The silently derived goal for an adopted worktree: the branch name verbatim,
/// unless the branch is generic — then the HEAD commit subject.
pub fn derive_adoption_goal(branch: &str, head_subject: &str) -> String {
    let segment = branch.rsplit('/').next().unwrap_or(branch).to_lowercase();
    let stem = strip_trailing_digit_run(&segment);
    let is_generic = stem.is_empty() || GENERIC_BRANCH_STEMS.contains(&stem.as_str());

    if !is_generic {
        return branch.to_string();
    }
    let subject = head_subject.trim();
    if !subject.is_empty() {
        subject.to_string()
    } else if !branch.is_empty() {
        branch.to_string()
    } else {
        "Adopted worktree".to_string()
    }
}

/// Strip one trailing run of ASCII digits, and the single `-`/`_` immediately
/// before that run, from a branch segment (`wip-2` -> `wip`, `test_3` -> `test`).
fn strip_trailing_digit_run(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut end = chars.len();
    while end > 0 && chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == chars.len() {
        return segment.to_string();
    }
    if end > 0 && (chars[end - 1] == '-' || chars[end - 1] == '_') {
        end -= 1;
    }
    chars[..end].iter().collect()
}

/// Enumerate every git worktree of `repo_path` that is neither the primary
/// checkout nor in `excluded_paths` (canonical paths of Build-bound worktrees —
/// runs and live planning worktrees, which must never surface as adoptable),
/// with a review summary per worktree. Read-only. A worktree whose summary
/// cannot be computed (corrupt checkout, no merge base with the base branch)
/// is skipped with an eprintln! — one broken stray must not fail the scan.
pub fn discover_external_worktrees(
    repo_path: &Path,
    base_branch: &str,
    excluded_paths: &HashSet<PathBuf>,
) -> Result<Vec<ExternalWorktree>, WorktreeError> {
    let output = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()?;
    if !output.status.success() {
        return Err(WorktreeError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    let primary_canonical = std::fs::canonicalize(repo_path)?;
    let repo = git2::Repository::open(repo_path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut found = Vec::new();
    for block in stdout.split("\n\n") {
        let Some(entry) = parse_worktree_block(
            block,
            &repo,
            &primary_canonical,
            excluded_paths,
            base_branch,
            now,
        ) else {
            continue;
        };
        found.push(entry);
    }

    found.sort_by(|a, b| {
        a.head_age_seconds
            .cmp(&b.head_age_seconds)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

/// Parse one `git worktree list --porcelain` block into an [`ExternalWorktree`],
/// or `None` if it should be skipped (bare/prunable, primary checkout, excluded,
/// gone from disk, or a summary that could not be computed — each case logs its
/// own `eprintln!` except the deliberately silent structural skips).
fn parse_worktree_block(
    block: &str,
    repo: &git2::Repository,
    primary_canonical: &Path,
    excluded_paths: &HashSet<PathBuf>,
    base_branch: &str,
    now: i64,
) -> Option<ExternalWorktree> {
    let block = block.trim();
    if block.is_empty() {
        return None;
    }

    let mut path = None;
    let mut head_sha = None;
    let mut branch = None;
    let mut detached = false;
    let mut bare = false;
    let mut prunable = false;
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            head_sha = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            branch = Some(rest.to_string());
        } else if line == "detached" {
            detached = true;
        } else if line == "bare" {
            bare = true;
        } else if line.starts_with("prunable") {
            prunable = true;
        }
    }
    if bare || prunable {
        return None;
    }
    let path = path?;
    if !path.exists() {
        return None;
    }
    let canonical_path = std::fs::canonicalize(&path).ok()?;
    if canonical_path == primary_canonical {
        return None;
    }
    if excluded_paths.contains(&canonical_path) {
        return None;
    }
    let head_sha = head_sha?;
    if !detached && branch.is_none() {
        // Malformed block: neither a branch nor an explicit detached marker.
        return None;
    }

    let name = resolve_worktree_name(repo, &canonical_path).or_else(|| {
        eprintln!(
            "discover_external_worktrees: no git worktree name for {}",
            canonical_path.display()
        );
        None
    })?;

    let head_oid = git2::Oid::from_str(&head_sha)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: bad HEAD sha for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;
    let commit = repo
        .find_commit(head_oid)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: no commit {head_sha} for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;
    let head_subject = commit.summary().unwrap_or("").to_string();
    let head_age_seconds = (now - commit.time().seconds()).max(0) as u64;

    let dirty_files = worktree_status_line_count(&canonical_path)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: status failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?;

    let diffstat = crate::diff::diff_against_merge_base(&canonical_path, base_branch)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: diff failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?
        .stat();

    // What is sitting in this tree unsaved — the +/− the rail shows. Distinct
    // from the diffstat above, which is everything the branch carries.
    let uncommitted = crate::diff::diff_uncommitted(&canonical_path)
        .inspect_err(|e| {
            eprintln!(
                "discover_external_worktrees: uncommitted diff failed for {}: {e}",
                canonical_path.display()
            );
        })
        .ok()?
        .stat();

    let (sync_base, ahead, behind) = sync_counts(repo, &commit, branch.as_deref(), base_branch);

    Some(ExternalWorktree {
        id: external_worktree_id(&canonical_path),
        name,
        path: canonical_path,
        branch,
        head_sha,
        head_subject,
        head_age_seconds,
        dirty_files,
        ahead,
        behind,
        sync_base,
        diffstat,
        uncommitted,
    })
}

/// How far this worktree has diverged, and from what.
///
/// A branch that tracks something is measured against its upstream: the thing
/// the human wants to know about `feature/x` tracking `origin/feature/x` is
/// whether it is pushed, not how it compares to main. A branch that tracks
/// nothing has only the base branch to be measured by — the branch it will
/// eventually merge into.
fn sync_counts(
    repo: &git2::Repository,
    head: &git2::Commit,
    branch: Option<&str>,
    base_branch: &str,
) -> (Option<String>, Option<u64>, Option<u64>) {
    if let Some((name, oid)) = branch.and_then(|b| upstream_of(repo, b)) {
        if let Ok((ahead, behind)) = repo.graph_ahead_behind(head.id(), oid) {
            return (Some(name), Some(ahead as u64), Some(behind as u64));
        }
    }
    repo.revparse_single(base_branch)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .and_then(|base| repo.graph_ahead_behind(head.id(), base.id()).ok())
        .map(|(ahead, behind)| {
            (
                Some(base_branch.to_string()),
                Some(ahead as u64),
                Some(behind as u64),
            )
        })
        .unwrap_or((None, None, None))
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

/// Match a canonicalized worktree path against git's own worktree registry to
/// recover the name `WorktreeManager` and `repo.find_worktree` expect.
fn resolve_worktree_name(repo: &git2::Repository, canonical_path: &Path) -> Option<String> {
    let names = repo.worktrees().ok()?;
    for name in names.iter().flatten() {
        let Ok(candidate) = repo.find_worktree(name) else {
            continue;
        };
        if std::fs::canonicalize(candidate.path()).ok().as_deref() == Some(canonical_path) {
            return Some(name.to_string());
        }
    }
    None
}

/// Count of non-empty `git status --porcelain` lines in `worktree_path`.
fn worktree_status_line_count(worktree_path: &Path) -> Result<usize, WorktreeError> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()?;
    if !output.status.success() {
        return Err(WorktreeError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    /// Init a repo on `main` with one commit, returning (tempdir, repo_path).
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@build.ing"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    fn manager(dir: &tempfile::TempDir, repo: &Path) -> WorktreeManager {
        WorktreeManager::new(repo, dir.path().join("worktrees"))
    }

    #[test]
    fn slugify_is_branch_safe() {
        assert_eq!(
            slugify("Fix the typo in the README"),
            "fix-the-typo-in-the-readme"
        );
        assert_eq!(slugify("  Add OAuth!! support  "), "add-oauth-support");
        assert_eq!(slugify("***"), "task");
        assert_eq!(slugify(""), "task");
        assert!(slugify(&"x".repeat(200)).len() <= 50);
        assert!(!slugify("trailing punctuation...").ends_with('-'));
    }

    #[test]
    fn branch_prefix_override_cuts_branches_in_that_namespace() {
        let (dir, repo) = init_repo();
        let plan_mgr = WorktreeManager::new(&repo, dir.path().join("worktrees"))
            .with_branch_prefix(PLAN_BRANCH_PREFIX);

        let plan_wt = plan_mgr.create("fix-typo", "main").unwrap();
        assert_eq!(plan_wt.branch, "plan/fix-typo");
        assert!(plan_wt.path.join("README.md").exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(r
            .find_branch("plan/fix-typo", git2::BranchType::Local)
            .is_ok());

        // A build-prefix worktree of the same slug shares the name/dir space,
        // so it disambiguates instead of colliding with the plan worktree.
        let build_mgr = manager(&dir, &repo);
        let build_wt = build_mgr.create("fix-typo", "main").unwrap();
        assert_eq!(build_wt.name, "fix-typo-2");
        assert_eq!(build_wt.branch, "build/fix-typo-2");

        // Teardown works across prefixes (remove never re-derives the branch).
        plan_mgr.remove(&plan_wt, /* keep_branch */ false).unwrap();
        assert!(!plan_wt.path.exists());
        assert!(r
            .find_branch("plan/fix-typo", git2::BranchType::Local)
            .is_err());
    }

    #[test]
    fn create_makes_branch_and_working_dir() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let wt = mgr.create("fix-typo", "main").unwrap();

        assert_eq!(wt.branch, "build/fix-typo");
        assert_eq!(wt.base_branch, "main");
        assert!(wt.path.join("README.md").exists(), "worktree has the files");

        // The branch exists in the repo.
        let r = git2::Repository::open(&repo).unwrap();
        assert!(r
            .find_branch("build/fix-typo", git2::BranchType::Local)
            .is_ok());
        // And the worktree is registered.
        assert!(r.worktrees().unwrap().iter().any(|n| n == Some("fix-typo")));
    }

    #[test]
    fn two_worktrees_on_one_repo_are_independent() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);

        let a = mgr.create("task-a", "main").unwrap();
        let b = mgr.create("task-b", "main").unwrap();

        assert_ne!(a.path, b.path);
        assert!(a.path.join("README.md").exists());
        assert!(b.path.join("README.md").exists());

        // A change in one worktree's branch does not appear in the other.
        std::fs::write(a.path.join("only-a.txt"), "a").unwrap();
        assert!(!b.path.join("only-a.txt").exists());
    }

    #[test]
    fn create_disambiguates_on_collision() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let a = mgr.create("dup", "main").unwrap();
        let b = mgr.create("dup", "main").unwrap();
        let c = mgr.create("dup", "main").unwrap();
        assert_eq!(a.name, "dup");
        assert_eq!(b.name, "dup-2");
        assert_eq!(c.name, "dup-3");
        assert_eq!(b.branch, "build/dup-2");
        assert!(b.path.join("README.md").exists());
    }

    #[test]
    fn abandon_removes_worktree_keeps_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("keep-me", "main").unwrap();

        mgr.remove(&wt, /* keep_branch */ true).unwrap();

        assert!(!wt.path.exists(), "working dir removed");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(!r.worktrees().unwrap().iter().any(|n| n == Some("keep-me")));
        assert!(
            r.find_branch("build/keep-me", git2::BranchType::Local)
                .is_ok(),
            "branch kept"
        );
    }

    #[test]
    fn remove_without_keep_deletes_branch() {
        let (dir, repo) = init_repo();
        let mgr = manager(&dir, &repo);
        let wt = mgr.create("drop-me", "main").unwrap();

        mgr.remove(&wt, /* keep_branch */ false).unwrap();

        assert!(!wt.path.exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("build/drop-me", git2::BranchType::Local)
                .is_err(),
            "branch deleted"
        );
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn external_worktree_id_is_stable_and_prefixed() {
        let a = PathBuf::from("/Users/zech/Projects/8ly/Build");
        let b = PathBuf::from("/Users/zech/Projects/8ly/Build-hotfix");

        let id_a1 = external_worktree_id(&a);
        let id_a2 = external_worktree_id(&a);
        let id_b = external_worktree_id(&b);

        assert_eq!(id_a1, id_a2);
        assert_ne!(id_a1, id_b);
        assert!(id_a1.starts_with("wt-"));
        assert_eq!(id_a1.len(), 15);
    }

    #[test]
    fn discovery_lists_a_user_worktree_and_skips_the_primary() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-a");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "hotfix/thing",
            ],
        );
        std::fs::write(wt_path.join("dirty.txt"), "dirty\n").unwrap();

        let excluded = std::collections::HashSet::new();
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

        assert_eq!(found.len(), 1);
        let entry = &found[0];
        assert_eq!(entry.branch, Some("hotfix/thing".to_string()));
        assert_eq!(entry.dirty_files, 1);
        assert!(!entry.head_subject.is_empty());
        assert_eq!(entry.name, "wt-a");
        assert!(entry.id.starts_with("wt-"));

        let primary_canonical = std::fs::canonicalize(&repo).unwrap();
        assert!(found.iter().all(|w| w.path != primary_canonical));
    }

    #[test]
    fn discovery_separates_what_is_uncommitted_from_what_the_branch_carries() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-mixed");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "mixed"],
        );
        // One committed line, then two uncommitted ones on top of it.
        std::fs::write(wt_path.join("committed.txt"), "one\n").unwrap();
        git_in(&wt_path, &["add", "committed.txt"]);
        git_in(&wt_path, &["commit", "-m", "committed work"]);
        std::fs::write(wt_path.join("dirty.txt"), "two\nthree\n").unwrap();

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

        let entry = &found[0];
        // The branch delta carries both; the uncommitted stat only what is
        // sitting in the tree unsaved.
        assert_eq!(entry.diffstat.insertions, 3);
        assert_eq!(entry.uncommitted.insertions, 2);
        assert_eq!(entry.uncommitted.files_changed, 1);
    }

    #[test]
    fn a_tracking_branch_is_measured_against_its_upstream() {
        let (dir, repo) = init_repo();
        let remote = dir.path().join("origin.git");
        git_in(&repo, &["init", "--bare", remote.to_str().unwrap()]);
        git_in(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );

        let wt_path = dir.path().join("wt-tracked");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "tracked",
            ],
        );
        git_in(&wt_path, &["push", "-u", "origin", "tracked"]);
        // Two commits past what was pushed — ahead of the upstream, not of main.
        for n in ["a", "b"] {
            std::fs::write(wt_path.join(format!("{n}.txt")), "x\n").unwrap();
            git_in(&wt_path, &["add", "."]);
            git_in(&wt_path, &["commit", "-m", n]);
        }

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

        let entry = &found[0];
        assert_eq!(entry.sync_base.as_deref(), Some("origin/tracked"));
        assert_eq!(entry.ahead, Some(2));
        assert_eq!(entry.behind, Some(0));
    }

    #[test]
    fn an_untracked_branch_falls_back_to_the_base_branch() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-untracked");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "solo"],
        );
        std::fs::write(wt_path.join("a.txt"), "x\n").unwrap();
        git_in(&wt_path, &["add", "."]);
        git_in(&wt_path, &["commit", "-m", "a"]);

        let found = discover_external_worktrees(&repo, "main", &HashSet::new()).unwrap();

        let entry = &found[0];
        assert_eq!(entry.sync_base.as_deref(), Some("main"));
        assert_eq!(entry.ahead, Some(1));
        assert_eq!(entry.behind, Some(0));
    }

    #[test]
    fn discovery_excludes_bound_paths() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-bound");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "bound/thing",
            ],
        );

        let mut excluded = std::collections::HashSet::new();
        excluded.insert(std::fs::canonicalize(&wt_path).unwrap());
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

        assert!(found.is_empty());
    }

    #[test]
    fn discovery_reports_detached_head() {
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-d");
        git_in(
            &repo,
            &["worktree", "add", "--detach", wt_path.to_str().unwrap()],
        );

        let excluded = std::collections::HashSet::new();
        let found = discover_external_worktrees(&repo, "main", &excluded).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].branch, None);
    }

    #[test]
    fn discovery_cache_invalidation_sees_new_head() {
        // Not a cache test (Layer 1 owns no cache) — confirms a fresh scan after
        // a new commit reflects the moved HEAD, the property the app-layer cache
        // invalidation relies on.
        let (dir, repo) = init_repo();
        let wt_path = dir.path().join("wt-c");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "feature/thing",
            ],
        );
        let excluded = std::collections::HashSet::new();
        let before = discover_external_worktrees(&repo, "main", &excluded).unwrap();
        let sha_before = before[0].head_sha.clone();

        std::fs::write(wt_path.join("more.txt"), "more\n").unwrap();
        git_in(&wt_path, &["add", "more.txt"]);
        git_in(&wt_path, &["commit", "-m", "more work"]);

        let after = discover_external_worktrees(&repo, "main", &excluded).unwrap();
        assert_ne!(before[0].head_sha, after[0].head_sha);
        assert_ne!(sha_before, after[0].head_sha);
    }

    #[test]
    fn derive_adoption_goal_pinned_cases() {
        assert_eq!(
            derive_adoption_goal("hotfix/login-redirect", "irrelevant"),
            "hotfix/login-redirect"
        );
        assert_eq!(
            derive_adoption_goal("wip", "Fix the thing"),
            "Fix the thing"
        );
        assert_eq!(
            derive_adoption_goal("wip-2", "some subject"),
            "some subject"
        );
        assert_eq!(
            derive_adoption_goal("zech/test_3", "some subject"),
            "some subject"
        );
        assert_eq!(
            derive_adoption_goal("feature", "some subject"),
            "some subject"
        );
        assert_eq!(derive_adoption_goal("", ""), "Adopted worktree");
    }
}
