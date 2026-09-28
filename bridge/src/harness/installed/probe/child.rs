//! One probe child: an installed CLI run by name, with nothing it says
//! trusted and nothing it starts left running.
//!
//! Its output is untrusted input read into memory, so it is bounded twice —
//! per line and in total — and a child that says more is ended. Its whole
//! process group is killed when the probe is done with it, whatever it
//! answered: the CLIs sit behind wrapper scripts that start children of their
//! own.

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use super::PROBE_DEADLINE;
use crate::harness::INHERITED_AGENT_MARKERS;

/// The longest line kept. A `model/list` page is about 10 KiB today.
pub(super) const MAX_LINE: usize = 1024 * 1024;

/// The most a probe child may say in all.
pub(super) const MAX_OUTPUT: usize = 4 * MAX_LINE;

/// Why a probe child stopped being read.
#[derive(Debug)]
enum Unread {
    TooLong,
    Failed(std::io::Error),
}

pub(super) struct ProbeChild {
    child: Child,
    lines: Receiver<Result<String, Unread>>,
    expiry: Instant,
}

impl ProbeChild {
    /// `binary args…`, looked up on `PATH` like a spawn, never through a
    /// shell. Started in the home directory: a version manager reads the
    /// directory it starts in for a project-local pin, and the answer wanted
    /// is the machine's. Stdin is a pipe only for a child that is talked to.
    pub(super) fn start(binary: &str, args: &[&str], talks: bool) -> std::io::Result<Self> {
        let mut child = command(binary, args, talks).spawn()?;
        let lines = read_lines(child.stdout.take());
        Ok(Self {
            child,
            lines,
            expiry: Instant::now() + PROBE_DEADLINE,
        })
    }

    pub(super) fn stdin(&mut self) -> std::io::Result<&mut ChildStdin> {
        self.child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("the probe's stdin is closed"))
    }

    /// The next line it said, `None` once it has said everything, or why it
    /// cannot be read: the deadline passed, or it said too much.
    pub(super) fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let left = self.expiry.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(left) {
            Ok(Ok(line)) => Ok(Some(line)),
            Ok(Err(Unread::TooLong)) => Err(std::io::Error::other(format!(
                "said more than {MAX_LINE} bytes in a line or {MAX_OUTPUT} in all"
            ))),
            Ok(Err(Unread::Failed(error))) => Err(error),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
            Err(RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer within {}s", PROBE_DEADLINE.as_secs()),
            )),
        }
    }

    /// Whether it exited successfully, waiting no longer than the deadline.
    pub(super) fn succeeded(&mut self) -> std::io::Result<bool> {
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status.success());
            }
            if Instant::now() >= self.expiry {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "did not exit in time",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// The command a probe runs. Offline to mise: a wrapper on `PATH` that runs
/// `mise use` or `mise x` installs the CLI when no version of it is, and a
/// probe is often the first thing to run on a new machine. Killed at the
/// deadline, that install is left half-downloaded until the next spawn
/// finishes it; offline, mise runs what is installed or fails at once, and a
/// probe that fails reads as knowing nothing.
pub(super) fn command(binary: &str, args: &[&str], talks: bool) -> Command {
    let mut command = Command::new(binary);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    for marker in INHERITED_AGENT_MARKERS {
        command.env_remove(marker);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    command
        .env("MISE_OFFLINE", "1")
        .args(args)
        .stdin(if talks { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .current_dir(home.unwrap_or_else(std::env::temp_dir));
    command
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        // SAFETY: a negative pid signals the process group this child leads,
        // which `process_group(0)` made it the leader of.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every line the child writes, each at most [`MAX_LINE`] and all of them at
/// most [`MAX_OUTPUT`], on a thread that ends with its output or the first
/// line past a bound.
fn read_lines(stdout: Option<ChildStdout>) -> Receiver<Result<String, Unread>> {
    let (lines, received) = std::sync::mpsc::channel();
    let Some(stdout) = stdout else {
        return received;
    };
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut said = 0;
        loop {
            let mut line = Vec::new();
            let read = (&mut reader)
                .take(MAX_LINE as u64 + 1)
                .read_until(b'\n', &mut line);
            let next = match read {
                Ok(0) => return,
                Ok(read) if read > MAX_LINE || said + read > MAX_OUTPUT => Err(Unread::TooLong),
                Ok(read) => {
                    said += read;
                    Ok(String::from_utf8_lossy(&line).trim_end().to_string())
                }
                Err(error) => Err(Unread::Failed(error)),
            };
            let last = next.is_err();
            if lines.send(next).is_err() || last {
                return;
            }
        }
    });
    received
}
