//! Running git as a child process.
//!
//! Every git child the isolation backends, the worktree façade and the
//! orchestrator start goes through here, so how a child is started, how its
//! streams are read and what a failure reads like are one fact with one owner.
//! This module knows nothing of worktrees, isolations or runs.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a git child may run before it is killed as timed out (spec §2).
const GIT_DEADLINE: Duration = Duration::from_secs(30);

/// How often the deadline is checked while the child runs. The pipes are read
/// by their own threads, so nothing but the kill waits on this.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

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
    let os_args: Vec<&OsStr> = args.iter().map(|arg| arg.as_ref()).collect();
    let out = run_git_with_deadline(dir, &os_args)?;
    if !out.status.success() {
        return Err(git_failure(&os_args, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What a git child that ran and failed said: the command, and everything on
/// both streams. Callers that read a raw [`Output`] turn a failure into an
/// error through here, so `From<GitError>` stays the one maker of a git-failure
/// sentence and no caller composes one of its own.
pub fn git_failure(args: &[&OsStr], out: &Output) -> GitError {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    GitError::Failed(format!("git {args:?}: {}", detail.join("\n")))
}

/// One git child in `dir` with terminal prompts disabled, both pipes drained,
/// and a 30 s deadline surfaced as [`std::io::ErrorKind::TimedOut`]. Callers
/// that need the raw `Output` (a fetch that may fail without git failing to
/// start) reach for this; `run_git` delegates to it, so a git child is spawned
/// in exactly one place.
pub fn run_git_with_deadline(dir: &Path, args: &[&OsStr]) -> std::io::Result<Output> {
    run_git_bounded(dir, args, GIT_DEADLINE)
}

fn run_git_bounded(dir: &Path, args: &[&OsStr], deadline: Duration) -> std::io::Result<Output> {
    let mut child = Command::new("git")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(dir)
        .spawn()?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let expiry = Instant::now() + deadline;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= expiry {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("git {args:?} did not return within {}s", deadline.as_secs()),
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    Ok(Output {
        status,
        stdout: collected(stdout)?,
        stderr: collected(stderr)?,
    })
}

/// Read one of the child's pipes on its own thread, so both are emptied while
/// the child is still writing. git blocks once a pipe buffer fills, so a reader
/// that waits for the exit first would wait for a child that is waiting for it.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut collected)?;
        }
        Ok(collected)
    })
}

/// Everything one drained pipe carried, once the child is done with it.
fn collected(reader: JoinHandle<std::io::Result<Vec<u8>>>) -> std::io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| std::io::Error::other("reading a git pipe panicked"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    /// A `git://` endpoint that completes the connection but never sends the
    /// ref advertisement, so a real `git fetch` against it blocks on read the
    /// way an unreachable server would — a git that never returns, with no
    /// environment mutation and no process left behind.
    fn stalling_git_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(held) => std::mem::forget(held),
                    Err(_) => break,
                }
            }
        });
        format!("git://127.0.0.1:{port}/never.git")
    }

    #[test]
    fn a_git_that_never_returns_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init"]).unwrap();
        let url = stalling_git_url();

        let started = Instant::now();
        let error = run_git_bounded(
            dir.path(),
            &[OsStr::new("fetch"), OsStr::new(url.as_str())],
            Duration::from_millis(500),
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
        assert!(started.elapsed() < Duration::from_secs(5), "was not killed");
    }

    /// A child that writes more than one pipe buffer must be drained while it
    /// runs: a reader that waits for the exit first deadlocks against a git
    /// blocked on a full pipe, and the deadline turns that into a timeout that
    /// has nothing to do with git being slow.
    #[test]
    fn output_larger_than_a_pipe_buffer_comes_back_whole() {
        let (_dir, repo) = crate::git_fixture::init_repo();
        let payload = "a line of output that fills the pipe\n".repeat(32768);
        std::fs::write(repo.join("big.txt"), &payload).unwrap();
        crate::git_fixture::git_in(&repo, &["add", "big.txt"]);
        crate::git_fixture::git_in(&repo, &["commit", "-m", "big"]);

        let started = Instant::now();
        let shown = run_git(&repo, &["show", "HEAD:big.txt"]).unwrap();

        assert_eq!(shown.len(), payload.len(), "the output came back truncated");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "draining waited for the exit: {:?}",
            started.elapsed()
        );
    }

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
