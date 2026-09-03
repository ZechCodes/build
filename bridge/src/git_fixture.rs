//! The repository every test that needs one starts from: one commit on
//! `main`, inside a temporary directory that lives as long as the test holds
//! it. One fixture, so a test about worktrees, isolation or orchestration
//! begins from the same repository and none of them describes it again.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A repository on `main` with one commit, as the directory that owns it and
/// the repository's own path inside it.
pub fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "test@build.ing"]);
    git_in(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "# project\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-m", "initial"]);
    (dir, repo)
}

/// Run one git command in `dir`, failing the test the moment git does.
pub fn git_in(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}
