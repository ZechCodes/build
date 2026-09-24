use std::collections::VecDeque;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::limits::{ProcessLimits, StderrRetention};
use crate::harness::HarnessError;
use crate::priority::ChildPlacement;
use crate::pty::HarnessSpec;

const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub type TerminalEventSink = Arc<dyn Fn(TerminalSourceEvent) + Send + Sync>;

/// A child pipe that the settle grace may settle on behalf of its reader or drainer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSource {
    Stdout,
    Stderr,
}

/// Everything the reaped child says about itself: its exit code, and the
/// failure of the monitor that watched for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub exit_code: Option<i32>,
    pub monitor_error: Option<String>,
}

/// Everything the drained stderr pipe says about itself: the retained tail, and
/// the failure of the drainer that read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StderrOutcome {
    pub retained_tail: Option<String>,
    pub drainer_error: Option<String>,
}

/// One settlement of the stdout reader, the process monitor, or the stderr drainer,
/// or the settle-grace expiry that settles a pipe still held open after the child was reaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSourceEvent {
    StdoutSettled {
        reader_error: Option<String>,
    },
    ProcessSettled(ProcessOutcome),
    StderrSettled(StderrOutcome),
    SourceExpired {
        source: TerminalSource,
        reason: String,
    },
}

pub struct ConnectionPipes {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

pub struct AppServerProcess {
    slot: Arc<ProcessSlot>,
}

struct ProcessSlot {
    state: Mutex<ProcessState>,
    reaped: Condvar,
}

struct ProcessState {
    child: Option<Box<dyn ChildControl>>,
    exit_code: Option<i32>,
    reaped: bool,
    kill_sent: bool,
    poll_failed: bool,
    process_error: Option<String>,
    shutdown_error: Option<String>,
}

trait ChildControl: Send {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
}

impl ChildControl for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
}

impl AppServerProcess {
    pub fn spawn(
        spec: &HarnessSpec,
        root: PathBuf,
        limits: ProcessLimits,
        events: TerminalEventSink,
    ) -> Result<(AppServerProcess, ConnectionPipes), HarnessError> {
        let binary = crate::pty::resolve_binary(spec)?;
        // Placed the way every child of the daemon is (`crate::priority`):
        // in a scope of its own behind the daemon and the user's apps where
        // the user's systemd answers, niced everywhere, before it runs a
        // thing — the same gate a terminal's child goes through.
        let mut child =
            ChildPlacement::current().spawn_command(spec.kind, &binary, &spec.args, |command| {
                command.current_dir(&root);
                for key in &spec.unset {
                    command.env_remove(key);
                }
                for (key, value) in &spec.env {
                    command.env(key, value);
                }
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Session("Codex stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Session("Codex stdout was not piped".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| HarnessError::Session("Codex stderr was not piped".to_string()))?;
        drain_stderr(
            stderr,
            StderrTail::new(limits.retention),
            Arc::clone(&events),
        );
        let process = AppServerProcess::new(Box::new(child));
        process.start_monitor(events, limits.source_settle_grace);
        Ok((process, ConnectionPipes { stdin, stdout }))
    }

    fn new(child: Box<dyn ChildControl>) -> AppServerProcess {
        AppServerProcess {
            slot: Arc::new(ProcessSlot {
                state: Mutex::new(ProcessState {
                    child: Some(child),
                    exit_code: None,
                    reaped: false,
                    kill_sent: false,
                    poll_failed: false,
                    process_error: None,
                    shutdown_error: None,
                }),
                reaped: Condvar::new(),
            }),
        }
    }

    fn start_monitor(&self, events: TerminalEventSink, source_settle_grace: Duration) {
        let slot = Arc::clone(&self.slot);
        std::thread::spawn(move || {
            let settled = loop {
                if let Some(settled) = poll_settlement(&slot) {
                    break settled;
                }
                std::thread::sleep(EXIT_POLL_INTERVAL);
            };
            events(settled);
            std::thread::sleep(source_settle_grace);
            for expiry in grace_expiries(source_settle_grace) {
                events(expiry);
            }
        });
    }

    pub fn exited_within(&self, timeout: Duration) -> bool {
        let state = self.slot.state.lock().unwrap();
        let (state, _) = self
            .slot
            .reaped
            .wait_timeout_while(state, timeout, |state| !state.reaped)
            .unwrap();
        state.reaped
    }

    pub fn shutdown(&self) -> Result<(), HarnessError> {
        let mut state = self.slot.state.lock().unwrap();
        if state.reaped {
            return cached_shutdown_result(&state);
        }
        let Some(mut child) = state.child.take() else {
            state.reaped = true;
            self.slot.reaped.notify_all();
            return cached_shutdown_result(&state);
        };

        let already_exited = if state.poll_failed {
            None
        } else {
            match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    record_process_error(&mut state, format!("Codex try_wait failed: {error}"));
                    None
                }
            }
        };
        if let Some(status) = already_exited {
            state.exit_code = Some(observed_code(status));
            state.reaped = true;
            self.slot.reaped.notify_all();
            return cached_shutdown_result(&state);
        }

        let kill_error = if !state.kill_sent {
            state.kill_sent = true;
            child.kill().err()
        } else {
            None
        };
        match child.wait() {
            Ok(status) => state.exit_code = Some(observed_code(status)),
            Err(error) => {
                let failure = match kill_error {
                    Some(kill_error) => {
                        format!("Codex kill failed: {kill_error}; Codex wait failed: {error}")
                    }
                    None => format!("Codex wait failed: {error}"),
                };
                record_process_error(&mut state, failure);
                state.child = Some(child);
                return cached_shutdown_result(&state);
            }
        }
        state.reaped = true;
        self.slot.reaped.notify_all();
        cached_shutdown_result(&state)
    }
}

fn poll_settlement(slot: &ProcessSlot) -> Option<TerminalSourceEvent> {
    let mut state = slot.state.lock().unwrap();
    if state.reaped || state.poll_failed {
        return Some(settled_event(&state));
    }
    match state.child.as_mut().map(|child| child.try_wait()) {
        Some(Ok(None)) => None,
        Some(Ok(Some(status))) => {
            state.exit_code = Some(observed_code(status));
            state.reaped = true;
            state.child = None;
            slot.reaped.notify_all();
            Some(settled_event(&state))
        }
        Some(Err(error)) => {
            state.poll_failed = true;
            record_process_error(&mut state, format!("Codex try_wait failed: {error}"));
            Some(settled_event(&state))
        }
        None => Some(settled_event(&state)),
    }
}

fn grace_expiries(source_settle_grace: Duration) -> [TerminalSourceEvent; 2] {
    [
        TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stdout,
            reason: format!("Codex stdout did not settle within {source_settle_grace:?}"),
        },
        TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stderr,
            reason: format!("Codex stderr did not settle within {source_settle_grace:?}"),
        },
    ]
}

fn settled_event(state: &ProcessState) -> TerminalSourceEvent {
    TerminalSourceEvent::ProcessSettled(ProcessOutcome {
        exit_code: state.exit_code,
        monitor_error: state.process_error.clone(),
    })
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn cached_shutdown_result(state: &ProcessState) -> Result<(), HarnessError> {
    match &state.shutdown_error {
        Some(error) => Err(HarnessError::Session(error.clone())),
        None => Ok(()),
    }
}

fn record_process_error(state: &mut ProcessState, error: String) {
    state.process_error.get_or_insert(error.clone());
    state.shutdown_error.get_or_insert(error);
}

fn observed_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

fn drain_stderr(reader: impl Read + Send + 'static, tail: StderrTail, events: TerminalEventSink) {
    std::thread::spawn(move || events(drain_stderr_reader(reader, tail)));
}

fn drain_stderr_reader(mut reader: impl Read, mut tail: StderrTail) -> TerminalSourceEvent {
    let mut buffer = [0_u8; 4096];
    let drainer_error = loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                tail.finish_line();
                break None;
            }
            Ok(read) => {
                for byte in &buffer[..read] {
                    tail.push(*byte);
                }
            }
            Err(error) => break Some(format!("Codex stderr read failed: {error}")),
        }
    };
    TerminalSourceEvent::StderrSettled(StderrOutcome {
        retained_tail: tail.epitaph(),
        drainer_error,
    })
}

struct StderrTail {
    line_limit: usize,
    total_limit: usize,
    current: Vec<u8>,
    retained: VecDeque<u8>,
}

impl StderrTail {
    fn new(retention: StderrRetention) -> StderrTail {
        StderrTail {
            line_limit: retention.line_bytes,
            total_limit: retention.total_bytes,
            current: Vec::with_capacity(retention.line_bytes),
            retained: VecDeque::with_capacity(retention.total_bytes),
        }
    }

    fn push(&mut self, byte: u8) {
        if byte == b'\n' {
            self.finish_line();
            return;
        }
        if self.current.len() < self.line_limit {
            self.current.push(byte);
        }
    }

    fn finish_line(&mut self) {
        if self.current.last() == Some(&b'\r') {
            self.current.pop();
        }
        if !self.current.is_empty() {
            if !self.retained.is_empty() {
                self.push_retained(b'\n');
            }
            let line = std::mem::replace(&mut self.current, Vec::with_capacity(self.line_limit));
            for byte in line {
                self.push_retained(byte);
            }
        }
        self.current.clear();
    }

    fn push_retained(&mut self, byte: u8) {
        if self.total_limit == 0 {
            return;
        }
        if self.retained.len() == self.total_limit {
            self.retained.pop_front();
        }
        self.retained.push_back(byte);
    }

    fn epitaph(&self) -> Option<String> {
        let bytes = self.retained.iter().copied().collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Error};
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::super::limits::AppServerLimits;
    use super::*;

    #[test]
    fn stderr_tail_caps_each_line_and_the_aggregate() {
        let mut tail = StderrTail::new(StderrRetention {
            line_bytes: 4,
            total_bytes: 7,
        });
        for byte in b"123456\nabc\ndef\n" {
            tail.push(*byte);
        }
        assert_eq!(tail.epitaph().as_deref(), Some("abc\ndef"));
        assert!(tail.retained.len() <= 7);
        assert!(tail.current.capacity() <= 4);
        assert!(tail.retained.capacity() <= 7);
    }

    #[test]
    fn shutdown_is_idempotent_and_reaps_the_child_exactly_once() {
        let root = tempfile::tempdir().unwrap();
        let spec = HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null");
        let (sender, receiver) = mpsc::channel();
        let events: TerminalEventSink = Arc::new(move |event| {
            let _ = sender.send(event);
        });
        let (process, pipes) = AppServerProcess::spawn(
            &spec,
            root.path().to_path_buf(),
            AppServerLimits::default().process(),
            events,
        )
        .unwrap();
        drop(pipes);
        process.shutdown().unwrap();
        process.shutdown().unwrap();
        assert!(process.exited_within(Duration::ZERO));
        let mut process_settlements = 0;
        while let Ok(event) = receiver.recv_timeout(Duration::from_secs(2)) {
            if matches!(event, TerminalSourceEvent::ProcessSettled(_)) {
                process_settlements += 1;
                break;
            }
        }
        assert_eq!(process_settlements, 1);
    }

    #[test]
    fn concurrent_shutdowns_kill_once_and_reap_once() {
        let calls = Arc::new(FakeCalls::default());
        let process = Arc::new(fake_process(FakeChild::running(Arc::clone(&calls))));
        let first_process = Arc::clone(&process);
        let first = std::thread::spawn(move || first_process.shutdown());
        let second_process = Arc::clone(&process);
        let second = std::thread::spawn(move || second_process.shutdown());
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        process.shutdown().unwrap();
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 1);
        drop(process);
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn signal_exit_has_a_stable_code() {
        assert_eq!(observed_code(ExitStatus::from_raw(9)), 137);
    }

    #[test]
    fn process_monitor_reports_try_wait_failure_without_status_polling() {
        let calls = Arc::new(FakeCalls::default());
        let process = fake_process(FakeChild::try_wait_error(Arc::clone(&calls)));
        let (sender, receiver) = mpsc::channel();
        let events: TerminalEventSink = Arc::new(move |event| {
            let _ = sender.send(event);
        });

        process.start_monitor(events, Duration::from_secs(5));

        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: None,
                monitor_error: Some(error),
            }) if error == "Codex try_wait failed: exact poll error"
        ));
        assert!(calls.polls.load(Ordering::SeqCst) > 0);
        assert!(process.shutdown().is_err());
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reap_lag_is_the_only_fact_a_caller_can_read_from_the_process() {
        let calls = Arc::new(FakeCalls::default());
        let process = fake_process(FakeChild::running(Arc::clone(&calls)));

        assert!(!process.exited_within(Duration::from_millis(10)));
        assert_eq!(calls.polls.load(Ordering::SeqCst), 0);
        process.shutdown().unwrap();
        assert!(process.exited_within(Duration::ZERO));
    }

    #[test]
    fn stderr_drainer_settles_with_its_read_failure() {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(Error::other("exact drain error"))
            }
        }

        assert!(matches!(
            drain_stderr_reader(FailedReader, StderrTail::new(AppServerLimits::default().process().retention)),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: None,
                drainer_error: Some(error),
            }) if error == "Codex stderr read failed: exact drain error"
        ));
    }

    #[test]
    fn stderr_drainer_settles_with_the_retained_tail() {
        assert!(matches!(
            drain_stderr_reader(Cursor::new(b"first\nsecond\n"), StderrTail::new(AppServerLimits::default().process().retention)),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some(tail),
                drainer_error: None,
            }) if tail == "first\nsecond"
        ));
    }

    #[test]
    fn failed_wait_keeps_the_child_for_a_retry_without_killing_twice() {
        let calls = Arc::new(FakeCalls::default());
        let process = fake_process(FakeChild::wait_error_once(Arc::clone(&calls)));

        assert!(process.shutdown().is_err());
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 1);
        assert!(process.shutdown().is_err());
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 2);
        assert!(process.exited_within(Duration::ZERO));
    }

    fn fake_process(child: FakeChild) -> AppServerProcess {
        AppServerProcess::new(Box::new(child))
    }

    #[derive(Default)]
    struct FakeCalls {
        polls: AtomicUsize,
        kills: AtomicUsize,
        waits: AtomicUsize,
    }

    struct FakeChild {
        calls: Arc<FakeCalls>,
        poll_error: bool,
        wait_error_once: bool,
    }

    impl FakeChild {
        fn running(calls: Arc<FakeCalls>) -> FakeChild {
            FakeChild {
                calls,
                poll_error: false,
                wait_error_once: false,
            }
        }

        fn try_wait_error(calls: Arc<FakeCalls>) -> FakeChild {
            FakeChild {
                calls,
                poll_error: true,
                wait_error_once: false,
            }
        }

        fn wait_error_once(calls: Arc<FakeCalls>) -> FakeChild {
            FakeChild {
                calls,
                poll_error: false,
                wait_error_once: true,
            }
        }
    }

    impl ChildControl for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
            self.calls.polls.fetch_add(1, Ordering::SeqCst);
            if self.poll_error {
                Err(Error::other("exact poll error"))
            } else {
                Ok(None)
            }
        }

        fn kill(&mut self) -> std::io::Result<()> {
            self.calls.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn wait(&mut self) -> std::io::Result<ExitStatus> {
            let call = self.calls.waits.fetch_add(1, Ordering::SeqCst);
            if self.wait_error_once && call == 0 {
                return Err(Error::other("exact wait error"));
            }
            Ok(ExitStatus::from_raw(0))
        }
    }
}
