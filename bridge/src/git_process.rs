//! Running git as a child process.
//!
//! Every git child the isolation backends, the worktree façade and the
//! orchestrator start goes through here — bar `bounded_git_fetch`, which keeps
//! its own deadline until `run_git_with_deadline` lands (spec §2) — so how a
//! child is started, how its streams are read and what a failure reads like
//! are one fact with one owner. This module knows nothing of worktrees,
//! isolations or runs.

use std::path::Path;
use std::process::Command;

/// Why one git child did not answer.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// git could not be started at all.
    #[error("{0}")]
    Unstartable(#[from] std::io::Error),
    /// git ran and failed, with everything it said about why.
    #[error("{0}")]
    Failed(String),
}

/// One git command in `dir`, its output or why it failed. git splits its story
/// across streams (a conflicting merge reports "CONFLICT …" on stdout), so a
/// failure carries both.
pub fn run_git(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let out = Command::new("git").args(args).current_dir(dir).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        return Err(GitError::Failed(format!(
            "git {args:?}: {}",
            detail.join("\n")
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_child_answers_with_its_stdout() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-b", "main"]).unwrap();

        let head = run_git(dir.path(), &["symbolic-ref", "--short", "HEAD"]).unwrap();

        assert_eq!(head.trim(), "main");
    }

    #[test]
    fn a_failure_carries_the_command_and_everything_git_said() {
        let dir = tempfile::tempdir().unwrap();

        let failure = run_git(dir.path(), &["rev-parse", "--is-inside-work-tree"])
            .unwrap_err()
            .to_string();

        assert!(failure.contains("rev-parse"), "{failure}");
        assert!(failure.contains("not a git repository"), "{failure}");
    }
}
