//! Reading one repository's Git state in a process of its own, so the reading
//! is bounded.
//!
//! `work_summary` (the test Done uses) runs a status, a history walk and a
//! diff inside libgit2, and none of them can be interrupted: on a large or
//! slow repository it runs as long as it runs. So the service never calls it
//! in the daemon. It starts the bridge again as `build-bridge measure-git`,
//! which reads one repository and prints what it found, and waits on it only
//! while the budget lasts. When the deadline passes or the daemon stops, the
//! reading is killed and reaped before the service goes on, and what it would
//! have said is not known: the workspace is held as `unmeasured`.

use super::budget::{Budget, Unfinished};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The repository a reading is of. An environment variable rather than an
/// argument, so the test binary can stand in for the bridge (its arguments
/// are its test harness's).
pub const REPOSITORY_VAR: &str = "BUILD_GIT_READING_REPOSITORY";

/// How the reading's one line of output begins.
const MARKER: &str = "BUILD-GIT-READING ";

/// How often a running reading is looked at.
const POLL: Duration = Duration::from_millis(5);

/// What one repository's Git state says, as the reading prints it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitReading {
    /// The repository could be read. When it could not, it is held as
    /// `unknown`, the way Done holds it.
    pub read: bool,
    pub pushes: u64,
    pub behind: u64,
    pub dirty: bool,
    pub dirty_files: u64,
    pub newest_commit_ms: Option<i64>,
}

/// The command a reading runs as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitProbe {
    program: PathBuf,
    args: Vec<OsString>,
}

impl Default for GitProbe {
    fn default() -> Self {
        Self::bridge()
    }
}

impl GitProbe {
    /// The running bridge, as `measure-git`. Under `cargo test` the test
    /// binary stands in, running only [`crate::reclaim::tests`]' reading.
    pub fn bridge() -> Self {
        let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("build-bridge"));
        #[cfg(not(test))]
        let args = vec![OsString::from("measure-git")];
        #[cfg(test)]
        let args = [
            "--exact",
            "reclaim::tests::git_reading_child",
            "--nocapture",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        Self { program, args }
    }

    /// Any command, for a test that needs a reading that never finishes.
    pub fn command(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
        }
    }

    /// Read `repository`, waiting only while `budget` lasts. `Err` when it
    /// ran out or the daemon stopped first; the reading has been killed and
    /// reaped by then. A reading that could not be started, or said nothing
    /// Build understands, is a repository that could not be read.
    pub fn read(&self, repository: &Path, budget: &Budget) -> Result<GitReading, Unfinished> {
        budget.check()?;
        let spawned = Command::new(&self.program)
            .args(&self.args)
            .env(REPOSITORY_VAR, repository)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                eprintln!(
                    "workspace reclaim: read {}: cannot start: {error}",
                    repository.display()
                );
                return Ok(GitReading::default());
            }
        };
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if budget.check().is_err() => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Unfinished);
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(GitReading::default());
                }
            }
        }
        let mut printed = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_string(&mut printed);
        }
        Ok(parse(&printed).unwrap_or_default())
    }
}

fn parse(printed: &str) -> Option<GitReading> {
    printed
        .lines()
        // Anywhere in a line: the test harness standing in for the bridge
        // prints the test's name ahead of it.
        .find_map(|line| line.split_once(MARKER).map(|(_, json)| json))
        .and_then(|json| serde_json::from_str(json).ok())
}

/// The reading's side: read the repository named by [`REPOSITORY_VAR`] and
/// answer the line to print.
pub fn reading_line() -> String {
    let reading = std::env::var_os(REPOSITORY_VAR)
        .map(|repository| read_here(Path::new(&repository)))
        .unwrap_or_default();
    format!(
        "{MARKER}{}",
        serde_json::to_string(&reading).unwrap_or_default()
    )
}

/// One repository's reading, in this process: Done's test, how many paths
/// `git status` would list, and when HEAD was committed.
fn read_here(repository: &Path) -> GitReading {
    let Ok(summary) = crate::gitgui::work_summary(repository) else {
        return GitReading::default();
    };
    GitReading {
        read: true,
        pushes: summary.pushes,
        behind: summary.behind,
        dirty: summary.dirty,
        dirty_files: dirty_file_count(repository),
        newest_commit_ms: head_commit_ms(repository),
    }
}

/// How many paths `git status` would list: edits, staged or not, and untracked
/// files. Ignored files are not work.
fn dirty_file_count(repository: &Path) -> u64 {
    let Ok(repo) = git2::Repository::open(repository) else {
        return 0;
    };
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    repo.statuses(Some(&mut options))
        .map(|statuses| statuses.len() as u64)
        .unwrap_or(0)
}

fn head_commit_ms(repository: &Path) -> Option<i64> {
    let repo = git2::Repository::open(repository).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit.time().seconds().saturating_mul(1000))
}
