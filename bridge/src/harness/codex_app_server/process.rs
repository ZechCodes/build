use std::collections::VecDeque;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::limits::AppServerLimits;
use crate::harness::HarnessError;
use crate::pty::HarnessSpec;

pub struct ConnectionPipes {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

pub struct AppServerProcess {
    state: Mutex<ProcessState>,
    stderr: Arc<Mutex<StderrTail>>,
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
        limits: &AppServerLimits,
    ) -> Result<(AppServerProcess, ConnectionPipes), HarnessError> {
        let mut command = Command::new(crate::pty::resolve_binary(spec)?);
        command.args(&spec.args).current_dir(root);
        for key in &spec.unset {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
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
        let retained = Arc::new(Mutex::new(StderrTail::new(
            limits.stderr_line_bytes,
            limits.stderr_total_bytes,
        )));
        drain_stderr(stderr, Arc::clone(&retained));
        Ok((
            AppServerProcess::new(Box::new(child), retained),
            ConnectionPipes { stdin, stdout },
        ))
    }

    fn new(child: Box<dyn ChildControl>, stderr: Arc<Mutex<StderrTail>>) -> AppServerProcess {
        AppServerProcess {
            state: Mutex::new(ProcessState {
                child: Some(child),
                exit_code: None,
                reaped: false,
                kill_sent: false,
                poll_failed: false,
                process_error: None,
                shutdown_error: None,
            }),
            stderr,
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        let mut state = self.state.lock().unwrap();
        if state.exit_code.is_some() || state.reaped || state.poll_failed {
            return state.exit_code;
        }
        let Some(child) = state.child.as_mut() else {
            return state.exit_code;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = observed_code(status);
                state.exit_code = Some(code);
                state.reaped = true;
                state.child = None;
                Some(code)
            }
            Ok(None) => None,
            Err(error) => {
                state.poll_failed = true;
                record_process_error(&mut state, format!("Codex try_wait failed: {error}"));
                None
            }
        }
    }

    pub fn liveness_failed(&self) -> bool {
        self.state.lock().unwrap().process_error.is_some()
            || self.stderr.lock().unwrap().read_error.is_some()
    }

    pub fn exited_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.exit_code().is_some() || self.liveness_failed() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn stderr_epitaph(&self) -> Option<String> {
        self.stderr.lock().unwrap().epitaph()
    }

    pub fn error_epitaph(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .process_error
            .clone()
            .or_else(|| self.stderr.lock().unwrap().read_error.clone())
    }

    pub fn shutdown(&self) -> Result<(), HarnessError> {
        let mut state = self.state.lock().unwrap();
        if state.reaped {
            return cached_shutdown_result(&state);
        }
        let Some(mut child) = state.child.take() else {
            state.reaped = true;
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
        cached_shutdown_result(&state)
    }
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

fn drain_stderr(reader: impl Read + Send + 'static, retained: Arc<Mutex<StderrTail>>) {
    std::thread::spawn(move || drain_stderr_reader(reader, retained));
}

fn drain_stderr_reader(mut reader: impl Read, retained: Arc<Mutex<StderrTail>>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                retained.lock().unwrap().finish_line();
                return;
            }
            Ok(read) => {
                let mut tail = retained.lock().unwrap();
                for byte in &buffer[..read] {
                    tail.push(*byte);
                }
            }
            Err(error) => {
                retained.lock().unwrap().read_error =
                    Some(format!("Codex stderr read failed: {error}"));
                return;
            }
        }
    }
}

struct StderrTail {
    line_limit: usize,
    total_limit: usize,
    current: Vec<u8>,
    retained: VecDeque<u8>,
    read_error: Option<String>,
}

impl StderrTail {
    fn new(line_limit: usize, total_limit: usize) -> StderrTail {
        StderrTail {
            line_limit,
            total_limit,
            current: Vec::with_capacity(line_limit),
            retained: VecDeque::with_capacity(total_limit),
            read_error: None,
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
    use std::io::Error;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn stderr_tail_caps_each_line_and_the_aggregate() {
        let mut tail = StderrTail::new(4, 7);
        for byte in b"123456\nabc\ndef\n" {
            tail.push(*byte);
        }
        assert_eq!(tail.epitaph().as_deref(), Some("abc\ndef"));
        assert!(tail.retained.len() <= 7);
        assert!(tail.current.capacity() <= 4);
        assert!(tail.retained.capacity() <= 7);
    }

    #[test]
    fn shutdown_is_idempotent_and_reaps_the_child() {
        let root = tempfile::tempdir().unwrap();
        let spec = HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null");
        let (process, pipes) = AppServerProcess::spawn(
            &spec,
            root.path().to_path_buf(),
            &AppServerLimits::default(),
        )
        .unwrap();
        drop(pipes);
        process.shutdown().unwrap();
        let first = process.exit_code();
        process.shutdown().unwrap();
        assert_eq!(process.exit_code(), first);
        assert!(first.is_some());
    }

    #[test]
    fn concurrent_status_and_shutdown_kill_once_and_reap_once() {
        let calls = Arc::new(FakeCalls::default());
        let process = Arc::new(fake_process(FakeChild::running(Arc::clone(&calls))));
        let status_process = Arc::clone(&process);
        let status = std::thread::spawn(move || status_process.exit_code());
        let end_process = Arc::clone(&process);
        let end = std::thread::spawn(move || end_process.shutdown());
        assert!(matches!(status.join().unwrap(), None | Some(0)));
        end.join().unwrap().unwrap();
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
    fn try_wait_and_stderr_read_errors_are_retained_exactly() {
        let calls = Arc::new(FakeCalls::default());
        let process = fake_process(FakeChild::try_wait_error(Arc::clone(&calls)));
        assert_eq!(process.exit_code(), None);
        assert_eq!(
            process.error_epitaph().as_deref(),
            Some("Codex try_wait failed: exact poll error")
        );
        assert!(process.exited_within(Duration::ZERO));
        assert!(process.shutdown().is_err());
        assert_eq!(calls.kills.load(Ordering::SeqCst), 1);
        assert_eq!(calls.waits.load(Ordering::SeqCst), 1);

        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(Error::other("exact drain error"))
            }
        }
        let retained = Arc::new(Mutex::new(StderrTail::new(16, 32)));
        drain_stderr_reader(FailedReader, Arc::clone(&retained));
        assert_eq!(
            retained.lock().unwrap().read_error.as_deref(),
            Some("Codex stderr read failed: exact drain error")
        );

        let process = AppServerProcess::new(
            Box::new(FakeChild::running(Arc::new(FakeCalls::default()))),
            retained,
        );
        assert!(process.liveness_failed());
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
        assert!(process.exit_code().is_some());
    }

    fn fake_process(child: FakeChild) -> AppServerProcess {
        AppServerProcess::new(
            Box::new(child),
            Arc::new(Mutex::new(StderrTail::new(16, 32))),
        )
    }

    #[derive(Default)]
    struct FakeCalls {
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
