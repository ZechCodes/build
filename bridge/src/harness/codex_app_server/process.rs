use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
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
    child: Mutex<Option<Child>>,
    exit_code: Mutex<Option<i32>>,
    stderr: Arc<Mutex<StderrTail>>,
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
            AppServerProcess {
                child: Mutex::new(Some(child)),
                exit_code: Mutex::new(None),
                stderr: retained,
            },
            ConnectionPipes { stdin, stdout },
        ))
    }

    pub fn exit_code(&self) -> Option<i32> {
        if let Some(code) = *self.exit_code.lock().unwrap() {
            return Some(code);
        }
        let observed = self
            .child
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
            .map(observed_code);
        if let Some(code) = observed {
            *self.exit_code.lock().unwrap() = Some(code);
        }
        observed
    }

    pub fn exited_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.exit_code().is_some() {
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

    pub fn shutdown(&self) -> Result<(), HarnessError> {
        if self.exit_code.lock().unwrap().is_some() {
            self.child.lock().unwrap().take();
            return Ok(());
        }
        let Some(mut child) = self.child.lock().unwrap().take() else {
            return Ok(());
        };
        let status = match child.try_wait()? {
            Some(status) => status,
            None => {
                let kill_error = child.kill().err();
                match child.wait() {
                    Ok(status) => status,
                    Err(wait_error) => {
                        return Err(HarnessError::Session(match kill_error {
                            Some(kill_error) => format!(
                                "could not kill Codex ({kill_error}) or reap it ({wait_error})"
                            ),
                            None => format!("could not reap Codex after killing it: {wait_error}"),
                        }))
                    }
                }
            }
        };
        *self.exit_code.lock().unwrap() = Some(observed_code(status));
        Ok(())
    }
}

impl Drop for AppServerProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn observed_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

fn drain_stderr(mut stderr: ChildStderr, retained: Arc<Mutex<StderrTail>>) {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match stderr.read(&mut buffer) {
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
                Err(_) => return,
            }
        }
    });
}

struct StderrTail {
    line_limit: usize,
    total_limit: usize,
    current: Vec<u8>,
    retained: Vec<u8>,
}

impl StderrTail {
    fn new(line_limit: usize, total_limit: usize) -> StderrTail {
        StderrTail {
            line_limit,
            total_limit,
            current: Vec::new(),
            retained: Vec::new(),
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
                self.retained.push(b'\n');
            }
            self.retained.extend_from_slice(&self.current);
            if self.retained.len() > self.total_limit {
                let drop_count = self.retained.len() - self.total_limit;
                self.retained.drain(..drop_count);
            }
        }
        self.current.clear();
    }

    fn epitaph(&self) -> Option<String> {
        let text = String::from_utf8_lossy(&self.retained).trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_tail_caps_each_line_and_the_aggregate() {
        let mut tail = StderrTail::new(4, 7);
        for byte in b"123456\nabc\ndef\n" {
            tail.push(*byte);
        }
        assert_eq!(tail.epitaph().as_deref(), Some("abc\ndef"));
        assert!(tail.retained.len() <= 7);
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
}
