//! Git worktree lifecycle: one worktree + branch per task.
//!
//! The bridge owns worktrees. A task gets an isolated branch (`build/<slug>`) and
//! a working directory cut from the project's base branch, so parallel tasks on
//! the same repo never touch each other. On abandon the worktree is removed but
//! the branch is kept (abandoning stays reversible-ish); merge decides for itself.

use std::path::PathBuf;

/// The branch-name prefix for every task branch: `build/<slug>`.
pub const BRANCH_PREFIX: &str = "build";

/// Things that can go wrong managing a worktree.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
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
}

impl WorktreeManager {
    /// `repo_path` is the project git repo; `worktrees_root` is where task
    /// worktrees are materialized (one subdirectory per task slug).
    pub fn new(repo_path: impl Into<PathBuf>, worktrees_root: impl Into<PathBuf>) -> Self {
        WorktreeManager {
            repo_path: repo_path.into(),
            worktrees_root: worktrees_root.into(),
        }
    }

    /// Create `build/<slug>` from `base_branch` and add a worktree for it. The name
    /// is made unique (`<slug>`, `<slug>-2`, …) so re-dispatching the same goal — or
    /// leftover branches/worktrees from prior tasks — never collides.
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
        let branch = branch_name(&name);

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
        repo.find_branch(&branch_name(name), git2::BranchType::Local)
            .is_ok()
            || repo.find_worktree(name).is_ok()
            || self.worktrees_root.join(name).exists()
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

/// Build the task branch name for a slug: `build/<slug>`.
fn branch_name(slug: &str) -> String {
    format!("{BRANCH_PREFIX}/{slug}")
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
}
