//! The repository every test that needs one starts from: one commit on
//! `main`, inside a temporary directory that lives as long as the test holds
//! it. One fixture, so a test about worktrees, isolation, orchestration, diffs
//! or the app begins from the same repository and none of them describes it
//! again.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What the one committed file says unless a test needs it to say otherwise.
const README: &str = "# project\n";

/// A repository called `repo` on `main` with one commit, as the directory that
/// owns it and the repository's own path inside it.
pub fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "repo");
    (dir, repo)
}

/// The same repository under `parent`, named. A project is named after its
/// repository directory, so a test about telling two projects apart says what
/// each one is called.
pub fn init_repo_named(parent: &Path, name: &str) -> PathBuf {
    init_repo_with_readme(parent, name, README)
}

/// The same repository with `README.md` saying `readme`, for a test whose
/// diffs are read against what that file held at the first commit.
pub fn init_repo_with_readme(parent: &Path, name: &str, readme: &str) -> PathBuf {
    let repo = parent.join(name);
    std::fs::create_dir(&repo).unwrap();
    git_in(&repo, &["init", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "test@build.ing"]);
    git_in(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), readme).unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-m", "initial"]);
    repo
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
