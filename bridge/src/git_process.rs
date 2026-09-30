//! Running git as a child process.
//!
//! Every git child the isolation backends, the worktree façade and the
//! orchestrator start goes through here, so how a child is started, how its
//! streams are read and what a failure reads like are one fact with one owner.
//! This module knows nothing of worktrees, isolations or runs.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long a git child may run before it is killed as timed out (spec §2).
const GIT_DEADLINE: Duration = Duration::from_secs(30);

/// How long a child that has closed its pipes but has not exited is waited for
/// by yielding rather than sleeping. Such a child is microseconds from gone,
/// and a host that rounds a short sleep up to a tenth of a second would charge
/// every git that raced its own exit that whole tenth.
const EXIT_SPIN: Duration = Duration::from_millis(2);

/// The gap between two looks at a child still running after [`EXIT_SPIN`]. It
/// is no longer finishing, so what the host rounds this up to is its own.
const EXIT_POLL: Duration = Duration::from_millis(5);

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
    run_command_with_deadline(OsStr::new("git"), dir, args, deadline)
}

/// Run a Git or isolation-provider executable directly, with bounded execution
/// and concurrent pipe draining. Arguments are never interpreted by a shell.
pub(crate) fn run_command_with_deadline(
    executable: &OsStr,
    dir: &Path,
    args: &[&OsStr],
    deadline: Duration,
) -> std::io::Result<Output> {
    run_with_environment(executable, dir, args, deadline, &[])
}

/// What a git child needs so that nothing asks anyone anything: no askpass
/// program for a username, password or passphrase, whether git's own
/// (`GIT_ASKPASS` set but empty outranks `core.askPass` and `SSH_ASKPASS`) or
/// ssh's (`SSH_ASKPASS_REQUIRE=never`, which a desktop session's `DISPLAY`
/// would otherwise switch on). With the terminal prompt already off, a
/// remote that wants a secret fails instead of waiting.
const UNATTENDED: &[(&str, &str)] = &[("GIT_ASKPASS", ""), ("SSH_ASKPASS_REQUIRE", "never")];

/// A git child nobody is at the keyboard for: a background fetch. Everything
/// [`run_command_with_deadline`] promises, and no prompt of any kind.
pub(crate) fn run_git_unattended(
    dir: &Path,
    args: &[&OsStr],
    deadline: Duration,
) -> std::io::Result<Output> {
    run_with_environment(OsStr::new("git"), dir, args, deadline, UNATTENDED)
}

fn run_with_environment(
    executable: &OsStr,
    dir: &Path,
    args: &[&OsStr],
    deadline: Duration,
    environment: &[(&str, &str)],
) -> std::io::Result<Output> {
    let mut command = Command::new(executable);
    command.envs(environment.iter().copied());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(dir)
        .spawn()?;
    bounded(child, executable, args, deadline)
}

/// Everything `child` said, or the deadline it did not answer within. Both
/// pipes are drained on threads of their own while it runs, and `deadline`
/// bounds the whole of it — the pipes reaching their end and the process
/// reaching its exit, which are two events and not one: a child that has let go
/// of both pipes and not yet exited is killed like any other.
fn bounded(
    mut child: Child,
    executable: &OsStr,
    args: &[&OsStr],
    deadline: Duration,
) -> std::io::Result<Output> {
    let (closed, pipe_closed) = std::sync::mpsc::channel();
    let stdout = drain(child.stdout.take(), closed.clone());
    let stderr = drain(child.stderr.take(), closed);
    let expiry = Instant::now() + deadline;
    for _ in 0..2 {
        let left = expiry.saturating_duration_since(Instant::now());
        if let Err(unread) = pipe_closed.recv_timeout(left) {
            kill_and_reap(&mut child);
            return Err(match unread {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    timed_out(executable, args, deadline, said_before_the_kill(stderr))
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::Error::other(format!(
                    "{executable:?} {args:?}: a pipe reader died before the child did"
                )),
            });
        }
    }
    let Some(status) = exit_before(&mut child, expiry)? else {
        kill_and_reap(&mut child);
        return Err(timed_out(
            executable,
            args,
            deadline,
            said_before_the_kill(stderr),
        ));
    };
    Ok(Output {
        status,
        stdout: collected(stdout)?,
        stderr: collected(stderr)?,
    })
}

/// End a child nobody is going to wait for, so no caller is answered while the
/// process it asked about is still running.
fn kill_and_reap(child: &mut Child) {
    // A provider can spawn Git helpers. End its whole process group so those
    // helpers cannot keep changing the checkout after a timeout is reported.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A child that outlived its deadline: what it was, and what it had said on
/// stderr by the time it was killed. A caller that wants to know why it hung
/// (a helper that says what it is waiting for, then waits) reads the latter with
/// [`said_before_deadline`]; every other caller sees only the sentence.
#[derive(Debug)]
struct DeadlinePassed {
    sentence: String,
    stderr: Vec<u8>,
}

impl std::fmt::Display for DeadlinePassed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.sentence)
    }
}

impl std::error::Error for DeadlinePassed {}

/// How a child that outlived its deadline is answered.
fn timed_out(
    executable: &OsStr,
    args: &[&OsStr],
    deadline: Duration,
    stderr: Vec<u8>,
) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        DeadlinePassed {
            sentence: format!(
                "{executable:?} {args:?} did not return within {}s",
                deadline.as_secs()
            ),
            stderr,
        },
    )
}

/// What a child that timed out had written to stderr, when `error` is such a
/// timeout.
pub(crate) fn said_before_deadline(error: &std::io::Error) -> Option<&[u8]> {
    error
        .get_ref()?
        .downcast_ref::<DeadlinePassed>()
        .map(|passed| passed.stderr.as_slice())
}

/// How long a killed child's stderr is waited for. Its whole process group
/// is gone, so the pipe ends at once, unless something that left the group
/// still holds it; that is not waited for.
const STDERR_AFTER_KILL: Duration = Duration::from_millis(250);

/// Everything a killed child wrote to stderr, if its pipe ends in time.
fn said_before_the_kill(stderr: JoinHandle<std::io::Result<Vec<u8>>>) -> Vec<u8> {
    let expiry = Instant::now() + STDERR_AFTER_KILL;
    while !stderr.is_finished() && Instant::now() < expiry {
        std::thread::sleep(EXIT_POLL);
    }
    if stderr.is_finished() {
        collected(stderr).unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// What the child exited with, or `None` when it is still running at `expiry`.
/// The first look is free, and a child that has let go of both its pipes has
/// all but exited — so only one still finishing pays a wait, and it pays it in
/// slices rather than in one unbounded `wait`.
fn exit_before(child: &mut Child, expiry: Instant) -> std::io::Result<Option<ExitStatus>> {
    let spinning_until = Instant::now() + EXIT_SPIN;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= expiry {
            return Ok(None);
        }
        if now < spinning_until {
            std::thread::yield_now();
        } else {
            std::thread::sleep(EXIT_POLL);
        }
    }
}

/// Read one of the child's pipes on its own thread, so both are emptied while
/// the child is still writing, and say on `closed` when it ends. git blocks
/// once a pipe buffer fills, so a reader that waits for the exit first would
/// wait for a child that is waiting for it; and a pipe reaching its end is the
/// child letting go of it, which is the moment the deadline is waiting for —
/// asked for by waiting on it rather than by looking every so often, so a git
/// costs what git costs and not what the host rounds a sleep up to.
fn drain<R: Read + Send + 'static>(
    pipe: Option<R>,
    closed: std::sync::mpsc::Sender<()>,
) -> JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let read = match pipe {
            Some(mut pipe) => pipe.read_to_end(&mut collected).map(|_| ()),
            None => Ok(()),
        };
        let _ = closed.send(());
        read.map(|()| collected)
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
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn provider_arguments_are_literal_and_failures_keep_the_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let argument = "a path; $(touch should-not-exist)";
        let output = run_command_with_deadline(
            OsStr::new("sh"),
            dir.path(),
            &[
                OsStr::new("-c"),
                OsStr::new("printf '%s' \"$1\"; printf 'provider error' >&2; exit 7"),
                OsStr::new("test-provider"),
                OsStr::new(argument),
            ],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, argument.as_bytes());
        assert_eq!(output.stderr, b"provider error");
        assert!(!dir.path().join("should-not-exist").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_timed_out_provider_cannot_leave_a_helper_modifying_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let error = run_command_with_deadline(
            OsStr::new("sh"),
            dir.path(),
            &[
                OsStr::new("-c"),
                OsStr::new("(sleep 0.3; touch late-write) & wait"),
            ],
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        std::thread::sleep(Duration::from_millis(400));
        assert!(!dir.path().join("late-write").exists());
    }

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

    /// Pipe EOF and a process exit are different events. A child that has let
    /// go of both pipes but has not exited — still fsyncing, still reaping a
    /// helper of its own — is killed at the deadline like any other: waiting on
    /// it is the unbounded wait every caller was promised this module makes
    /// instead of them.
    #[test]
    fn a_child_that_closes_its_pipes_and_lingers_is_killed_at_the_deadline() {
        let child = Command::new("sh")
            .args(["-c", "exec 1>&- 2>&-; sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let started = Instant::now();
        let error = bounded(
            child,
            OsStr::new("sh"),
            &[OsStr::new("linger")],
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut, "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the child was waited on past its deadline: {:?}",
            started.elapsed()
        );
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

    /// The child is answered when it exits, not when something notices it has.
    /// A sleeping poll costs whatever the host's timers round it up to — on a
    /// machine that rounds a 5 ms sleep to 180 ms, every git the daemon runs
    /// pays that, and a `done` report holding the app mutex over three of them
    /// reads as a wedged daemon. Measured against the same child run by the
    /// standard library, so the assertion is about this module's overhead
    /// rather than about how fast the host runs git.
    #[test]
    fn a_git_child_costs_what_the_child_costs() {
        let dir = tempfile::tempdir().unwrap();
        run_git(dir.path(), &["init", "-b", "main"]).unwrap();
        let args = ["symbolic-ref", "--short", "HEAD"];

        let started = Instant::now();
        for _ in 0..5 {
            Command::new("git")
                .args(args)
                .stdin(Stdio::null())
                .current_dir(dir.path())
                .output()
                .unwrap();
        }
        let library = started.elapsed();
        let started = Instant::now();
        for _ in 0..5 {
            run_git(dir.path(), &args).unwrap();
        }
        let ours = started.elapsed();

        assert!(
            ours < library * 3,
            "the wait costs more than the child: {ours:?} against {library:?}"
        );
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
