//! One probe child: an installed CLI run by name, with nothing it says
//! trusted and nothing it starts left running.
//!
//! Its output is untrusted input read into memory, so it is bounded twice —
//! per line and in total — and a child that says more is ended. Its whole
//! process group is killed when the probe is done with it, whatever it
//! answered: the CLIs sit behind wrapper scripts that start children of their
//! own.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::PROBE_DEADLINE;
use crate::harness::{DAEMON_IDENTITY_VARS, INHERITED_AGENT_MARKERS};

/// The longest line kept. A `model/list` page is about 10 KiB today.
pub(super) const MAX_LINE: usize = 1024 * 1024;

/// The most stdout a probe child may say in all.
pub(super) const MAX_OUTPUT: usize = 4 * MAX_LINE;

/// Stderr is drained independently of stdout, keeping only a bounded tail
/// of its last nonempty line. A noisy wrapper cannot fill its stderr pipe.
const MAX_STDERR_BYTES: usize = 4096;
const MAX_STDERR_TEXT: usize = 512;
const MAX_STDERR_WAIT: Duration = Duration::from_millis(50);

/// Why a probe child stopped being read.
#[derive(Debug)]
enum Unread {
    TooLong,
    Failed(std::io::Error),
}

pub(super) struct ProbeChild {
    child: Child,
    lines: Receiver<Result<String, Unread>>,
    stderr: Stderr,
    /// Answers stop early enough to drain final stderr within the deadline.
    expiry: Instant,
    hard_expiry: Instant,
    stopped: bool,
}

impl ProbeChild {
    /// `binary args…`, looked up on `PATH` like a spawn, never through a
    /// shell. Started in the home directory: a version manager reads the
    /// directory it starts in for a project-local pin, and the answer wanted
    /// is the machine's. Stdin is a pipe only for a child that is talked to.
    pub(super) fn start(binary: &str, args: &[&str], talks: bool) -> std::io::Result<Self> {
        ProbeChild::start_within(binary, args, talks, PROBE_DEADLINE)
    }

    /// The same, cut off at `deadline` instead of [`PROBE_DEADLINE`].
    pub(super) fn start_within(
        binary: &str,
        args: &[&str],
        talks: bool,
        deadline: Duration,
    ) -> std::io::Result<Self> {
        let hard_expiry = Instant::now() + deadline;
        let diagnostic_reserve = MAX_STDERR_WAIT.min(deadline / 10);
        let mut child = command(binary, args, talks).spawn()?;
        let lines = read_lines(child.stdout.take());
        let stderr = Stderr::capture(child.stderr.take());
        Ok(Self {
            child,
            lines,
            stderr,
            expiry: hard_expiry - diagnostic_reserve,
            hard_expiry,
            stopped: false,
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

    /// Stop before collecting the diagnostic: EOF on stdout may arrive
    /// before the stderr reader has consumed the child's final error.
    pub(super) fn failure(&mut self, error: std::io::Error) -> std::io::Error {
        self.stop();
        let left = self.hard_expiry.saturating_duration_since(Instant::now());
        let stderr = self.stderr.last(left.min(MAX_STDERR_WAIT));
        if stderr.is_empty() {
            return error;
        }
        std::io::Error::new(error.kind(), format!("{error}; stderr: {stderr}"))
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
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
    for marker in INHERITED_AGENT_MARKERS
        .into_iter()
        .chain(DAEMON_IDENTITY_VARS)
    {
        command.env_remove(marker);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    command
        .env("MISE_OFFLINE", "1")
        .args(args)
        .stdin(if talks { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(home.unwrap_or_else(std::env::temp_dir));
    command
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct StderrReading {
    last: String,
    finished: bool,
}

#[derive(Clone, Default)]
struct Stderr(Arc<StderrState>);

#[derive(Default)]
struct StderrState {
    reading: Mutex<StderrReading>,
    finished: Condvar,
    #[cfg(test)]
    before_wait: Mutex<Option<std::sync::mpsc::Sender<Duration>>>,
}

impl Stderr {
    fn capture(stderr: Option<ChildStderr>) -> Self {
        let saved = Self::default();
        let Some(stderr) = stderr else {
            saved.finish();
            return saved;
        };
        let reading = saved.clone();
        std::thread::spawn(move || {
            drain_stderr(stderr, &reading);
            reading.finish();
        });
        saved
    }

    fn remember(&self, line: &mut VecDeque<u8>) {
        let text = String::from_utf8_lossy(line.make_contiguous());
        let mut chars: Vec<_> = text
            .chars()
            .rev()
            .filter(|c| !c.is_control())
            .take(MAX_STDERR_TEXT)
            .collect();
        chars.reverse();
        let text: String = chars.into_iter().collect();
        let text = text.trim();
        if !text.is_empty() {
            self.0.reading.lock().unwrap().last = text.to_owned();
        }
    }

    fn finish(&self) {
        self.0.reading.lock().unwrap().finished = true;
        self.0.finished.notify_all();
    }

    fn last(&self, wait: Duration) -> String {
        let reading = self.0.reading.lock().unwrap();
        #[cfg(test)]
        if let Some(started) = self.0.before_wait.lock().unwrap().take() {
            let _ = started.send(wait);
        }
        let (reading, _) = self
            .0
            .finished
            .wait_timeout_while(reading, wait, |reading| !reading.finished)
            .unwrap();
        reading.last.clone()
    }
}

fn drain_stderr(mut stderr: ChildStderr, saved: &Stderr) {
    let mut bytes = [0; MAX_STDERR_BYTES];
    let mut line = VecDeque::with_capacity(MAX_STDERR_BYTES);
    while let Ok(read) = stderr.read(&mut bytes) {
        if read == 0 {
            break;
        }
        remember_stderr_chunk(&mut line, &bytes[..read], saved);
    }
    saved.remember(&mut line);
}

fn remember_stderr_chunk(line: &mut VecDeque<u8>, bytes: &[u8], saved: &Stderr) {
    for &byte in bytes {
        if byte == b'\n' {
            saved.remember(line);
            line.clear();
        } else {
            if line.len() == MAX_STDERR_BYTES {
                line.pop_front();
            }
            line.push_back(byte);
        }
    }
    // Keep a partial line too, so a deadline still has the latest reason.
    saved.remember(line);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_leaves_time_for_a_delayed_stderr_collector() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("hanging");
        crate::isolation::test_fixture::write_executable(&cli, "#!/bin/sh\nsleep 60\n");
        let mut child = ProbeChild::start_within(
            cli.to_str().unwrap(),
            &["--version"],
            false,
            Duration::from_millis(500),
        )
        .unwrap();

        // Publish only when diagnostic collection starts, simulating a
        // stderr reader still draining the child's last bytes after timeout.
        let delayed = Stderr::default();
        let (started, waiting) = std::sync::mpsc::channel();
        *delayed.0.before_wait.lock().unwrap() = Some(started);
        child.stderr = delayed.clone();
        let collector = std::thread::spawn(move || {
            let wait = waiting.recv().unwrap();
            let mut line = VecDeque::from(b"late-timeout-reason".to_vec());
            delayed.remember(&mut line);
            delayed.finish();
            wait
        });

        let timeout = child.next_line().unwrap_err();
        assert_eq!(timeout.kind(), std::io::ErrorKind::TimedOut);
        let error = child.failure(timeout);
        let wait = collector.join().unwrap();
        assert!(
            !wait.is_zero(),
            "timeout left no diagnostic collection time"
        );
        assert!(wait <= Duration::from_millis(50));
        assert!(error.to_string().contains("late-timeout-reason"), "{error}");
    }
}
