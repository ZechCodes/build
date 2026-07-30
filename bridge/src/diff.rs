//! Git is the integration layer. The bridge never asks a harness what it did —
//! the worktree knows.
//!
//! This module renders `git diff` of a task's worktree against its base branch in
//! two shapes: a cheap **summary** (files changed, +/- lines) for the quiet
//! progress state while building, and the **full patch** for the review gate. It
//! also answers the plan-phase enforcement question — *did anything change
//! outside `.build/`?* — so the UI can flag a planning agent that wrote code.

use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

/// Where plan-phase work is supposed to stay confined.
pub const PLAN_SCOPE_PREFIX: &str = ".build/";

/// Roll-up counts for the quiet progress state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// How a single path changed relative to base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Other,
}

/// One changed path in the worktree's delta from base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangeStatus,
}

/// The worktree's complete delta from its base branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDiff {
    stat: DiffStat,
    files: Vec<ChangedFile>,
    patch: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
}

impl WorktreeDiff {
    /// The roll-up counts (the building-state progress fact).
    pub fn stat(&self) -> DiffStat {
        self.stat
    }

    /// The changed files, in diff order.
    pub fn files(&self) -> &[ChangedFile] {
        &self.files
    }

    /// The full unified patch (the review-gate surface).
    pub fn patch(&self) -> &str {
        &self.patch
    }

    /// Changed paths that fall outside `prefix`. Used for plan-phase enforcement
    /// by observation: a planning agent should only touch `.build/`.
    pub fn paths_outside<'a>(&'a self, prefix: &str) -> Vec<&'a str> {
        self.files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| !p.starts_with(prefix))
            .collect()
    }

    /// Whether the planning agent strayed outside `.build/`.
    pub fn touched_outside_plan_scope(&self) -> bool {
        !self.paths_outside(PLAN_SCOPE_PREFIX).is_empty()
    }
}

/// Compute the worktree's diff against `base_branch`.
///
/// Compares the base branch's tree to the worktree's working directory *and*
/// index, so it captures committed, staged, and unstaged changes alike — the
/// total delta a reviewer should see, regardless of how the agent committed.
pub fn diff_against_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let base_tree = repo.revparse_single(base_branch)?.peel_to_tree()?;
    diff_tree_to_dirty_workdir(&repo, Some(&base_tree))
}

/// The worktree's total delta from its fork point with `base_branch`: the
/// merge-base tree vs the working directory *and* index, untracked included —
/// committed, staged, unstaged, and new files together. This is the browse/
/// review surface for external worktrees, which may long predate the base tip;
/// `diff_against_base` (anchored on the run's `base_sha`) remains the run-diff
/// surface.
pub fn diff_against_merge_base(
    worktree_path: &Path,
    base_branch: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let base_commit = repo.revparse_single(base_branch)?.peel_to_commit()?;
    let head_commit = repo.head()?.peel_to_commit()?;
    let merge_base_id = repo.merge_base(base_commit.id(), head_commit.id())?;
    let merge_base_tree = repo.find_commit(merge_base_id)?.tree()?;
    diff_tree_to_dirty_workdir(&repo, Some(&merge_base_tree))
}

/// Render an immutable commit-to-commit range. Inputs must be full object ids,
/// not revspecs: callers resolve only persisted stage boundaries through this
/// helper, so later HEAD movement and dirty files cannot alter the result.
pub fn diff_between_commits(
    worktree_path: &Path,
    start_sha: &str,
    completion_sha: &str,
) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(worktree_path)?;
    let start = repo.find_commit(git2::Oid::from_str(start_sha)?)?;
    let completion = repo.find_commit(git2::Oid::from_str(completion_sha)?)?;
    let start_tree = start.tree()?;
    let completion_tree = completion.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&start_tree), Some(&completion_tree), None)?;
    worktree_diff_from_git_diff(&diff)
}

/// Shared tail of both diff entry points: `old_tree` vs the worktree's dirty
/// working directory and index (untracked included).
/// The scaffolded per-owner MCP config: machine-local plumbing, never the
/// user's work — excluded from every review surface.
pub(crate) const MCP_CONFIG_PATH: &str = ".build/mcp.json";

fn delta_path(delta: &git2::DiffDelta) -> String {
    delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn diff_tree_to_dirty_workdir(
    repo: &git2::Repository,
    old_tree: Option<&git2::Tree>,
) -> Result<WorktreeDiff, DiffError> {
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let diff = repo.diff_tree_to_workdir_with_index(old_tree, Some(&mut opts))?;
    worktree_diff_from_git_diff(&diff)
}

fn worktree_diff_from_git_diff(diff: &git2::Diff<'_>) -> Result<WorktreeDiff, DiffError> {
    let files: Vec<ChangedFile> = diff
        .deltas()
        .map(|delta| ChangedFile {
            path: delta_path(&delta),
            status: map_status(delta.status()),
        })
        .filter(|file| file.path != MCP_CONFIG_PATH)
        .collect();

    // Stats are counted while printing (instead of `diff.stats()`) so the
    // excluded MCP config contributes to neither the patch nor the numbers.
    let mut insertions = 0;
    let mut deletions = 0;
    let mut patch = String::new();
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        if delta_path(&delta) == MCP_CONFIG_PATH {
            return true;
        }
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
    })?;
    let stat = DiffStat {
        files_changed: files.len(),
        insertions,
        deletions,
    };

    Ok(WorktreeDiff { stat, files, patch })
}

fn map_status(status: git2::Delta) -> ChangeStatus {
    match status {
        git2::Delta::Added | git2::Delta::Untracked | git2::Delta::Copied => ChangeStatus::Added,
        git2::Delta::Modified | git2::Delta::Typechange => ChangeStatus::Modified,
        git2::Delta::Deleted => ChangeStatus::Deleted,
        git2::Delta::Renamed => ChangeStatus::Renamed,
        _ => ChangeStatus::Other,
    }
}

/// A live, debounced stream of recomputed diffs for a worktree. Holds the fs
/// watcher and the worker thread alive; dropping it stops watching.
pub struct DiffWatcher {
    _watcher: notify::RecommendedWatcher,
}

/// Begin watching `worktree_path`; every burst of filesystem changes is debounced
/// by `debounce`, then a freshly recomputed [`WorktreeDiff`] is sent on the
/// returned channel. The first diff is sent immediately so subscribers start with
/// current state.
pub fn watch(
    worktree_path: &Path,
    base_branch: &str,
    debounce: Duration,
) -> Result<(DiffWatcher, mpsc::UnboundedReceiver<WorktreeDiff>), DiffError> {
    use notify::Watcher;

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })
    .map_err(notify_to_git)?;
    watcher
        .watch(worktree_path, notify::RecursiveMode::Recursive)
        .map_err(notify_to_git)?;

    let (diff_tx, diff_rx) = mpsc::unbounded_channel();

    // Send the current diff straight away so a subscriber starts from truth.
    if let Ok(initial) = diff_against_base(worktree_path, base_branch) {
        let _ = diff_tx.send(initial);
    }

    let worktree_path = worktree_path.to_path_buf();
    let base_branch = base_branch.to_string();
    std::thread::spawn(move || {
        // Block for the first event of a burst, then drain until quiet for
        // `debounce`, recompute once, and emit.
        while raw_rx.recv().is_ok() {
            while raw_rx.recv_timeout(debounce).is_ok() {}
            match diff_against_base(&worktree_path, &base_branch) {
                Ok(diff) => {
                    if diff_tx.send(diff).is_err() {
                        break; // receiver dropped
                    }
                }
                Err(_) => continue,
            }
        }
    });

    Ok((DiffWatcher { _watcher: watcher }, diff_rx))
}

/// notify and git2 errors don't share a type; carry the message through git2's.
fn notify_to_git(err: notify::Error) -> DiffError {
    DiffError::Git(git2::Error::from_str(&err.to_string()))
}

/// The primary checkout's uncommitted delta: HEAD's tree vs the working
/// directory and index, untracked included — staged + unstaged + new files.
/// This is the "main worktree" review surface; committed work is upstream's
/// business, not a review surface.
pub fn diff_against_head(repo_path: &Path) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = repo.head()?.peel_to_tree()?;
    diff_tree_to_dirty_workdir(&repo, Some(&head_tree))
}

/// Like [`diff_against_head`], but an unborn HEAD (a repo with no commits yet)
/// diffs against the empty tree instead of failing — the git-GUI status
/// surface must keep working in a brand-new repository.
pub fn diff_uncommitted(repo_path: &Path) -> Result<WorktreeDiff, DiffError> {
    let repo = git2::Repository::open(repo_path)?;
    let head_tree = match repo.head() {
        Ok(head) => Some(head.peel_to_tree()?),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            None
        }
        Err(e) => return Err(e.into()),
    };
    diff_tree_to_dirty_workdir(&repo, head_tree.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// A repo on `main` with one commit; returns (tempdir, repo_path).
    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@build.ing"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "# project\nline\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    #[test]
    fn no_changes_is_an_empty_diff() {
        let (_dir, repo) = init_repo();
        let diff = diff_against_base(&repo, "main").unwrap();
        assert_eq!(diff.stat(), DiffStat::default());
        assert!(diff.files().is_empty());
        assert!(diff.patch().is_empty());
    }

    #[test]
    fn untracked_and_modified_files_are_summarized() {
        let (_dir, repo) = init_repo();
        // Modify a tracked file and add a new untracked one.
        std::fs::write(repo.join("README.md"), "# project\nline\nadded\n").unwrap();
        std::fs::write(repo.join("new.txt"), "hello\nworld\n").unwrap();

        let diff = diff_against_base(&repo, "main").unwrap();
        let stat = diff.stat();
        assert_eq!(stat.files_changed, 2);
        assert!(stat.insertions >= 3, "got {stat:?}");

        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"new.txt"));
        // The patch is the full review surface.
        assert!(diff.patch().contains("new.txt"));
        assert!(diff.patch().contains("+hello"));
    }

    #[test]
    fn dirty_diff_excludes_the_scaffolded_mcp_config() {
        let (_dir, repo) = init_repo();
        // Adoption scaffolds the machine-local MCP config into the worktree; it
        // is plumbing, not the user's work, and must never reach the review
        // surface.
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
        std::fs::write(repo.join("visible.txt"), "real work\n").unwrap();

        let diff = diff_against_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"visible.txt"), "{paths:?}");
        assert!(!paths.contains(&".build/mcp.json"), "{paths:?}");
        assert_eq!(diff.stat().files_changed, 1, "{:?}", diff.stat());
        assert!(!diff.patch().contains("mcp.json"));
    }

    #[test]
    fn committed_changes_on_the_branch_are_included() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("feature.rs"), "fn main() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feature"]);

        // Even though it's committed, the delta from base must show it.
        let diff = diff_against_base(&repo, "main").unwrap();
        assert_eq!(diff.stat().files_changed, 1);
        assert_eq!(diff.files()[0].path, "feature.rs");
        assert_eq!(diff.files()[0].status, ChangeStatus::Added);
    }

    #[test]
    fn plan_scope_enforcement_flags_out_of_scope_writes() {
        let (_dir, repo) = init_repo();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/plan.md"), "# plan\n").unwrap();

        // Only `.build/` touched → nothing out of scope.
        let in_scope = diff_against_base(&repo, "main").unwrap();
        assert!(!in_scope.touched_outside_plan_scope());
        assert!(in_scope.paths_outside(PLAN_SCOPE_PREFIX).is_empty());

        // Now the planning agent writes code it shouldn't have.
        std::fs::write(repo.join("src.rs"), "code\n").unwrap();
        let strayed = diff_against_base(&repo, "main").unwrap();
        assert!(strayed.touched_outside_plan_scope());
        assert_eq!(strayed.paths_outside(PLAN_SCOPE_PREFIX), vec!["src.rs"]);
    }

    #[test]
    fn merge_base_diff_sees_committed_staged_unstaged_and_untracked() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("committed.txt"), "committed\n").unwrap();
        git(&["add", "committed.txt"]);
        git(&["commit", "-m", "committed"]);
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&["add", "staged.txt"]);
        std::fs::write(repo.join("README.md"), "# project\nline\nmodified\n").unwrap();
        std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"committed.txt"), "got {paths:?}");
        assert!(paths.contains(&"staged.txt"), "got {paths:?}");
        assert!(paths.contains(&"README.md"), "got {paths:?}");
        assert!(paths.contains(&"untracked.txt"), "got {paths:?}");
        assert!(diff.patch().contains("+untracked"));
    }

    #[test]
    fn merge_base_diff_ignores_base_movement() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["checkout", "-b", "build/x"]);
        std::fs::write(repo.join("feature.rs"), "fn main() {}\n").unwrap();
        git(&["add", "feature.rs"]);
        git(&["commit", "-m", "feature"]);

        // Advance main with an unrelated commit after the branch forked.
        git(&["checkout", "main"]);
        std::fs::write(repo.join("upstream.txt"), "upstream\n").unwrap();
        git(&["add", "upstream.txt"]);
        git(&["commit", "-m", "upstream"]);
        git(&["checkout", "build/x"]);

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        assert_eq!(diff.files().len(), 1);
        assert_eq!(diff.files()[0].path, "feature.rs");
    }

    #[test]
    fn merge_base_diff_on_detached_head_works() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        let head_sha = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        git(&["checkout", "--detach", &head_sha]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();

        let diff = diff_against_merge_base(&repo, "main").unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"dirty.txt"), "got {paths:?}");
    }

    #[test]
    fn commit_range_diff_is_stable_after_later_commits_and_dirty_changes() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output.status);
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        let start = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.join("stage-one.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "stage one"]);
        let completion = git(&["rev-parse", "HEAD"]);

        let expected = diff_between_commits(&repo, &start, &completion).unwrap();
        std::fs::write(repo.join("stage-two.txt"), "two\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "stage two"]);
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();
        let after = diff_between_commits(&repo, &start, &completion).unwrap();

        assert_eq!(after, expected);
        assert!(after.patch().contains("stage-one.txt"));
        assert!(!after.patch().contains("stage-two.txt"));
        assert!(!after.patch().contains("dirty.txt"));
    }

    #[test]
    fn diff_against_head_is_empty_on_a_clean_checkout() {
        let (_dir, repo) = init_repo();
        let diff = diff_against_head(&repo).unwrap();
        assert_eq!(diff.stat(), DiffStat::default());
        assert!(diff.files().is_empty());
        assert!(diff.patch().is_empty());
    }

    #[test]
    fn diff_against_head_counts_staged_unstaged_and_untracked() {
        let (_dir, repo) = init_repo();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        // Unstaged modification.
        std::fs::write(repo.join("README.md"), "# project\nline\nmodified\n").unwrap();
        // Staged new file.
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&["add", "staged.txt"]);
        // Untracked new file.
        std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();

        let diff = diff_against_head(&repo).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"README.md"), "got {paths:?}");
        assert!(paths.contains(&"staged.txt"), "got {paths:?}");
        assert!(paths.contains(&"untracked.txt"), "got {paths:?}");
        assert_eq!(diff.stat().files_changed, 3, "{:?}", diff.stat());
    }

    #[test]
    fn diff_against_head_excludes_the_scaffolded_mcp_config() {
        let (_dir, repo) = init_repo();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
        std::fs::write(repo.join("visible.txt"), "real work\n").unwrap();

        let diff = diff_against_head(&repo).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"visible.txt"), "{paths:?}");
        assert!(!paths.contains(&".build/mcp.json"), "{paths:?}");
        assert_eq!(diff.stat().files_changed, 1, "{:?}", diff.stat());
        assert!(!diff.patch().contains("mcp.json"));
    }

    #[tokio::test]
    async fn watcher_pushes_a_recomputed_diff_on_change() {
        let (dir, repo) = init_repo();
        // Keep the tempdir alive for the whole test.
        let _keep = &dir;

        let (_watcher, mut rx) = watch(&repo, "main", Duration::from_millis(50)).unwrap();

        // First message is the immediate baseline (empty).
        let initial = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("baseline diff arrives")
            .unwrap();
        assert_eq!(initial.stat(), DiffStat::default());

        // Touch a file; expect a debounced, recomputed diff that sees it.
        std::fs::write(repo.join("changed.txt"), "x\n").unwrap();
        let updated = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("change diff arrives")
            .unwrap();
        assert!(
            updated.files().iter().any(|f| f.path == "changed.txt"),
            "watcher should report the new file, got {:?}",
            updated.files()
        );
    }
}
