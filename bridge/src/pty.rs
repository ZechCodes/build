//! Full-PTY harness sessions — the one way Build talks *to* an agent.
//!
//! There is no harness SDK. Dispatching a phase means writing a prompt into the
//! agent's PTY; the user dropping in means attaching to the same PTY. A harness
//! adapter is just two things (scope §3): a spawn command and how to submit a
//! prompt. Output is broadcast to every subscriber (the relay stream, the
//! quiescence monitor) and the time of the last byte drives idle detection.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

/// Things that can go wrong driving a PTY.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("pty error: {0}")]
    Pty(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// How a harness wants a prompt "sent" once written. Most CLIs submit on Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitKey {
    /// Append a carriage return (the Enter key) after the prompt text.
    Enter,
    /// Write the prompt verbatim with no terminator.
    None,
}

impl SubmitKey {
    fn bytes(&self) -> &'static [u8] {
        match self {
            SubmitKey::Enter => b"\r",
            SubmitKey::None => b"",
        }
    }
}

/// The whole harness adapter contract: how to spawn it and how to submit a prompt.
#[derive(Debug, Clone)]
pub struct HarnessSpec {
    /// The binary to exec (e.g. `claude`, `codex`).
    pub binary: String,
    /// Arguments, including the YOLO flag (`--dangerously-skip-permissions`, etc.).
    pub args: Vec<String>,
    /// Extra environment (e.g. the per-task MCP config pointing at the `done` server).
    pub env: Vec<(String, String)>,
    /// How a written prompt is submitted.
    pub submit: SubmitKey,
}

impl HarnessSpec {
    /// A bare spec for `binary` with Enter-to-submit and no extra args/env.
    pub fn new(binary: impl Into<String>) -> Self {
        HarnessSpec {
            binary: binary.into(),
            args: Vec::new(),
            env: Vec::new(),
            submit: SubmitKey::Enter,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// A live agent session bound to a PTY. Cloneable handles share one underlying PTY.
pub struct PtySession {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    last_activity: Arc<Mutex<Instant>>,
    submit: SubmitKey,
}

impl PtySession {
    /// Spawn `spec` in a fresh PTY of `size`, optionally in `cwd` (the worktree).
    pub fn spawn(
        spec: &HarnessSpec,
        cwd: Option<PathBuf>,
        size: PtySize,
    ) -> Result<PtySession, PtyError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| PtyError::Pty(e.to_string()))?;

        let mut cmd = CommandBuilder::new(&spec.binary);
        cmd.args(&spec.args);
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }
        if let Some(cwd) = cwd {
            cmd.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Pty(e.to_string()))?;
        // Close the slave in the parent so EOF propagates when the child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Pty(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Pty(e.to_string()))?;

        let (output_tx, _) = broadcast::channel(1024);
        let last_activity = Arc::new(Mutex::new(Instant::now()));

        // Blocking reader pump: forward chunks and stamp activity. A dropped
        // receiver is fine (broadcast lag/closed is not fatal to the pump).
        {
            let output_tx = output_tx.clone();
            let last_activity = Arc::clone(&last_activity);
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            *last_activity.lock().unwrap() = Instant::now();
                            let _ = output_tx.send(buf[..n].to_vec());
                        }
                    }
                }
            });
        }

        Ok(PtySession {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            output_tx,
            last_activity,
            submit: spec.submit.clone(),
        })
    }

    /// Subscribe to the raw output stream. Each subscriber sees every chunk from
    /// the moment it subscribes.
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    /// Write a prompt and submit it (per the harness's `SubmitKey`).
    pub fn write_prompt(&self, prompt: &str) -> Result<(), PtyError> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(prompt.as_bytes())?;
        writer.write_all(self.submit.bytes())?;
        writer.flush()?;
        Ok(())
    }

    /// Write raw bytes to the PTY (user keystrokes from an attached terminal).
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), PtyError> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// How long since the PTY last produced output.
    pub fn idle_for(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.last_activity.lock().unwrap())
    }

    /// Resolve once the PTY has been silent for at least `threshold` — the
    /// quiescence signal that demotes to `idle_unreported` when no `done` arrives.
    pub async fn wait_quiescent(&self, threshold: Duration) {
        loop {
            let idle = self.idle_for();
            if idle >= threshold {
                return;
            }
            tokio::time::sleep(threshold - idle).await;
        }
    }

    /// Resize the terminal (an attached client changed its viewport).
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        self.master
            .lock()
            .unwrap()
            .resize(size)
            .map_err(|e| PtyError::Pty(e.to_string()))
    }

    /// Kill the harness process.
    pub fn kill(&self) -> Result<(), PtyError> {
        self.child.lock().unwrap().kill()?;
        Ok(())
    }

    /// Whether the harness process has exited (crash, completion, kill). Reaps the
    /// child if it has — `try_wait` collects the exit status — so polling this
    /// never leaves a zombie behind.
    pub fn has_exited(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(Some(_)))
    }

    /// The harness's OS process id, if it is still running.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().process_id()
    }

    /// Kill the harness and reap it. `kill` alone leaves a zombie: portable-pty's
    /// unix child does not reap on drop, so every phase transition on a long-lived
    /// daemon would otherwise leak one process-table entry.
    pub fn kill_and_reap(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Block until the harness exits, returning whether it exited successfully.
    pub fn wait(&self) -> Result<bool, PtyError> {
        Ok(self.child.lock().unwrap().wait()?.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_pty() -> PtySize {
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Collect output until `needle` appears or the deadline passes.
    async fn read_until(rx: &mut broadcast::Receiver<Vec<u8>>, needle: &str) -> String {
        let mut acc = String::new();
        let deadline = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(chunk) => {
                        acc.push_str(&String::from_utf8_lossy(&chunk));
                        if acc.contains(needle) {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                }
            }
        })
        .await;
        assert!(
            deadline.is_ok(),
            "timed out waiting for {needle:?}; got: {acc:?}"
        );
        acc
    }

    #[tokio::test]
    async fn prompt_roundtrips_through_the_pty() {
        // A tiny "harness" that reads one line and replies deterministically.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("read x; printf 'REPLY[%s]' \"$x\"");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();

        session.write_prompt("ping").unwrap();

        let out = read_until(&mut rx, "REPLY[ping]").await;
        assert!(out.contains("REPLY[ping]"), "got: {out:?}");
    }

    #[tokio::test]
    async fn quiescence_fires_after_silence() {
        // A harness that produces nothing, then sleeps.
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 5");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let waited = tokio::time::timeout(
            Duration::from_secs(2),
            session.wait_quiescent(Duration::from_millis(150)),
        )
        .await;

        assert!(
            waited.is_ok(),
            "quiescence should be detected during silence"
        );
        assert!(session.idle_for() >= Duration::from_millis(150));
        session.kill().unwrap();
    }

    #[tokio::test]
    async fn activity_keeps_it_non_quiescent() {
        // Emits a line every 50ms for a while: never quiescent at a 300ms bar.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("for i in 1 2 3 4 5 6 7 8; do printf 'tick\\n'; sleep 0.05; done; sleep 5");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();
        read_until(&mut rx, "tick").await;

        // While ticking, a 300ms quiescence bar must not be reached.
        let quiesced = tokio::time::timeout(
            Duration::from_millis(250),
            session.wait_quiescent(Duration::from_millis(300)),
        )
        .await;
        assert!(quiesced.is_err(), "should still be active while ticking");
        session.kill().unwrap();
    }

    #[tokio::test]
    async fn cwd_is_respected() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let spec = HarnessSpec::new("sh").arg("-c").arg("pwd");
        let session = PtySession::spawn(&spec, Some(canonical.clone()), small_pty()).unwrap();
        let mut rx = session.subscribe();

        let leaf = canonical.file_name().unwrap().to_string_lossy().to_string();
        let out = read_until(&mut rx, &leaf).await;
        assert!(out.contains(&leaf), "pwd should show the worktree: {out:?}");
    }

    #[tokio::test]
    async fn kill_then_wait_reports_failure() {
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        session.kill().unwrap();
        // A killed process does not exit successfully.
        assert!(!session.wait().unwrap());
    }
}
