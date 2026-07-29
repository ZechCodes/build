//! Full-PTY harness sessions — the one way Build talks *to* an agent.
//!
//! There is no harness SDK. Dispatching a phase means writing a prompt into the
//! agent's PTY; the user dropping in means attaching to the same PTY. A harness
//! adapter is just two things (scope §3): a spawn command and how to submit a
//! prompt. Output is broadcast to every subscriber (the relay stream, the
//! quiescence monitor) and the time of the last byte drives idle detection.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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
    #[error("harness {binary:?} not found in PATH {path:?} — install it, or restart the daemon from a shell that can see it")]
    HarnessNotFound { binary: String, path: String },
}

/// Resolve `binary` the way a shell would, against the daemon's PATH (or the
/// spec's own override). We do this rather than leaving it to portable-pty:
/// portable-pty searches the *CommandBuilder's* environment, which falls back to
/// confstr `_CS_PATH` ("/usr/bin:/bin:/usr/sbin:/sbin") and produces an opaque
/// "No viable candidates" error that names neither the harness nor the fix.
fn resolve_binary(spec: &HarnessSpec) -> Result<PathBuf, PtyError> {
    if spec.binary.contains('/') {
        return Ok(PathBuf::from(&spec.binary));
    }
    let path = spec
        .env
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    for dir in path.split(':').filter(|dir| !dir.is_empty()) {
        let candidate = PathBuf::from(dir).join(&spec.binary);
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(PtyError::HarnessNotFound {
        binary: spec.binary.clone(),
        path,
    })
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Bracketed-paste frame delimiters (xterm). A TUI receiving input between
/// these treats it as one pasted block instead of typed keystrokes, so the
/// embedded newlines of a multi-line prompt do not read as the Enter key and
/// submit the prompt as N fragmented turns.
const PASTE_START: &str = "\u{1b}[200~";
const PASTE_END: &str = "\u{1b}[201~";

/// DECSET 2004. A TUI emits this once its line editor is live and it wants
/// pastes bracketed — which is exactly the precondition for injecting a prompt:
/// it says both "input is being serviced" and "the frame below will be honored".
/// First output is a weaker signal (a harness paints its banner, or a modal
/// dialog, well before it will accept a turn).
const PASTE_MODE_ENABLED: &[u8] = b"\x1b[?2004h";

/// Default settle window for a harness that declares none — small, because a
/// scripted harness paints once and stops. Real TUIs override it: see
/// [`HarnessSpec::settle`].
const DEFAULT_SETTLE: Duration = Duration::from_millis(50);

/// Remove bracketed-paste markers from prompt text before it enters a TUI.
/// Prompt text carries reviewer-supplied thread content: an embedded paste-end
/// would close the frame mid-prompt and replay the remainder as raw keystrokes
/// (newlines as Enter included), and an embedded paste-start could open a
/// frame around later input.
///
/// Single pass, checking the OUTPUT tail after each push: removing a marker can
/// splice its neighbours into a fresh one, and re-examining the tail catches
/// that as it forms. Rescanning the whole string per removal instead would be
/// quadratic in nesting depth — and since this text is reviewer-supplied and
/// the dispatch holds the app-wide lock, that is a remote stall of every
/// project rather than merely a slow function.
fn strip_bracketed_paste_markers(prompt: &str) -> String {
    let mut sanitized = String::with_capacity(prompt.len());
    for character in prompt.chars() {
        sanitized.push(character);
        // Markers are pure ASCII, so truncating by their byte length always
        // lands on a character boundary.
        for marker in [PASTE_START, PASTE_END] {
            if sanitized.ends_with(marker) {
                sanitized.truncate(sanitized.len() - marker.len());
                break;
            }
        }
    }
    sanitized
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
    /// Environment variables to REMOVE before exec.
    ///
    /// Build spawns an agent from a process that may itself be an agent, and a
    /// harness that finds its own session markers in the environment believes it
    /// is a nested child of that session — claude 2.1.219 disables transcript
    /// saving and warns about an "inherited CLAUDE_CODE_CHILD_SESSION marker".
    /// The spawned agent must be its own session, never a continuation of
    /// whatever launched the daemon.
    pub unset: Vec<String>,
    /// How long this harness must stop painting before its input is live.
    ///
    /// Startup is a burst with GAPS in it — measured against claude 2.1.219:
    /// paste mode at 550ms, a 311ms lull, then the alternate-screen switch at
    /// 1212ms whose clear discards anything typed during that lull, and a final
    /// render at 1761ms. So the window must outlast the largest intra-startup
    /// gap, not merely the first pause. Per-harness rather than global: a
    /// scripted test harness paints once and would otherwise pay a real TUI's
    /// startup cost on every spawn.
    pub settle: Duration,
}

impl HarnessSpec {
    /// A bare spec for `binary` with Enter-to-submit and no extra args/env.
    pub fn new(binary: impl Into<String>) -> Self {
        HarnessSpec {
            binary: binary.into(),
            args: Vec::new(),
            env: Vec::new(),
            submit: SubmitKey::Enter,
            unset: Vec::new(),
            settle: DEFAULT_SETTLE,
        }
    }

    /// Remove `key` from the spawned harness's environment.
    pub fn unset(mut self, key: impl Into<String>) -> Self {
        self.unset.push(key.into());
        self
    }

    /// Remove every named key from the spawned harness's environment.
    pub fn unset_all<K: Into<String>>(mut self, keys: impl IntoIterator<Item = K>) -> Self {
        self.unset.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Declare how long this harness's startup paint takes to settle.
    pub fn settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
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
    /// The output broadcast sender, dropped by the reader thread at PTY EOF so
    /// every subscriber observes `Closed` when the child exits — even while the
    /// session itself is still held (the keyed-terminal pumps key off this).
    output_tx: Arc<Mutex<Option<broadcast::Sender<Vec<u8>>>>>,
    last_activity: Arc<Mutex<Instant>>,
    /// Set by the reader pump when the child enables bracketed-paste mode — the
    /// readiness signal [`ready_within`](Self::ready_within) waits on before the
    /// first prompt write. `last_activity` cannot express this: it is stamped
    /// "now" at spawn, so it never distinguishes "not ready" from "just spawned".
    accepts_paste: Arc<AtomicBool>,
    submit: SubmitKey,
    settle: Duration,
    /// The child's exit code, cached the first time it is observed. `try_wait`
    /// reaps the child exactly once, so the status must be remembered here or the
    /// crash-detection message ("exit code N") could never recover the code after
    /// the first poll.
    exit_code: Mutex<Option<i32>>,
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

        let mut cmd = CommandBuilder::new(resolve_binary(spec)?);
        cmd.args(&spec.args);
        for key in &spec.unset {
            cmd.env_remove(key);
        }
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

        let (sender, _) = broadcast::channel(1024);
        let output_tx = Arc::new(Mutex::new(Some(sender.clone())));
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let accepts_paste = Arc::new(AtomicBool::new(false));

        // Blocking reader pump: forward chunks and stamp activity. A dropped
        // receiver is fine (broadcast lag/closed is not fatal to the pump). At
        // EOF the thread drops BOTH senders (the slot's and its own), so every
        // subscriber sees `Closed` the moment the child exits.
        {
            let output_slot = Arc::clone(&output_tx);
            let last_activity = Arc::clone(&last_activity);
            let accepts_paste = Arc::clone(&accepts_paste);
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut buf = [0u8; 4096];
                // The paste-mode sequence can straddle a read boundary, so each
                // scan is prefixed with the tail of the previous chunk.
                let mut carry: Vec<u8> = Vec::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            *last_activity.lock().unwrap() = Instant::now();
                            if !accepts_paste.load(Ordering::Relaxed) {
                                carry.extend_from_slice(&buf[..n]);
                                if carry
                                    .windows(PASTE_MODE_ENABLED.len())
                                    .any(|window| window == PASTE_MODE_ENABLED)
                                {
                                    accepts_paste.store(true, Ordering::Relaxed);
                                    carry = Vec::new();
                                } else {
                                    let keep = carry.len().min(PASTE_MODE_ENABLED.len() - 1);
                                    carry.drain(..carry.len() - keep);
                                }
                            }
                            let _ = sender.send(buf[..n].to_vec());
                        }
                    }
                }
                output_slot.lock().unwrap().take();
            });
        }

        Ok(PtySession {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            output_tx,
            last_activity,
            accepts_paste,
            submit: spec.submit.clone(),
            settle: spec.settle,
            exit_code: Mutex::new(None),
        })
    }

    /// Subscribe to the raw output stream. Each subscriber sees every chunk from
    /// the moment it subscribes, and observes `Closed` once the PTY hits EOF.
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        match self.output_tx.lock().unwrap().as_ref() {
            Some(tx) => tx.subscribe(),
            None => {
                // The PTY already hit EOF: hand back an already-closed stream.
                let (tx, rx) = broadcast::channel(1);
                drop(tx);
                rx
            }
        }
    }

    /// Write a prompt and submit it (per the harness's `SubmitKey`). A
    /// multi-line prompt bound for an Enter-submitting TUI travels as ONE
    /// bracketed paste: written raw, the TUI would read every embedded newline
    /// as the Enter key and submit the prompt as fragmented turns. Prompts
    /// written verbatim (`SubmitKey::None`) are never framed — that contract
    /// promises the harness the exact bytes.
    pub fn write_prompt(&self, prompt: &str) -> Result<(), PtyError> {
        let sanitized = strip_bracketed_paste_markers(prompt);
        let mut writer = self.writer.lock().unwrap();
        if self.submit == SubmitKey::Enter && sanitized.contains('\n') {
            writer.write_all(PASTE_START.as_bytes())?;
            writer.write_all(sanitized.as_bytes())?;
            writer.write_all(PASTE_END.as_bytes())?;
        } else {
            writer.write_all(sanitized.as_bytes())?;
        }
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

    #[cfg(test)]
    pub(crate) fn set_idle_for_test(&self, idle: Duration) {
        *self.last_activity.lock().unwrap() = Instant::now() - idle;
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
        self.exit_code().is_some()
    }

    /// Whether the child signals it will accept a bracketed paste within
    /// `timeout` — the readiness signal a fresh prompt write waits on.
    ///
    /// First output is NOT that signal, and the difference is the whole point:
    /// a harness paints its banner (or a modal workspace-trust dialog) long
    /// before its line editor will take a turn, so a prompt written on first
    /// byte lands in whatever owns the keyboard at the time and the submit key
    /// answers it. [`PASTE_MODE_ENABLED`] is emitted by the line editor itself,
    /// so it says both "input is being serviced" and "the frame will be
    /// honored". Returns `false` (promptly, not at the deadline) for a child
    /// that exits first: it will never become ready, and the caller's exit-race
    /// guard should see the write failure without extra delay.
    pub fn ready_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            // Paste mode alone is necessary but NOT sufficient: a real TUI
            // announces it during startup and then switches to the alternate
            // screen and clears it, which wipes anything typed in between.
            // Verified against claude 2.1.219 — the prompt wrote successfully,
            // and the clear discarded it. So also require the paint storm to
            // have settled: an editor that has stopped redrawing is one that is
            // waiting on input.
            if self.accepts_paste.load(Ordering::Relaxed) && self.idle_for() >= self.settle {
                return true;
            }
            if self.has_exited() || Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Whether the child exits within `timeout`. `has_exited` is a single
    /// racy poll: a dying harness closes its side of the PTY (so writes fail
    /// with EIO) *before* the OS makes its exit status reapable, so one poll
    /// can report a harness that is already gone as still running. Callers
    /// deciding whether a PTY write error means "crashed" (benign) rather
    /// than "wedged" (fatal) wait out that reap lag here instead.
    pub fn exited_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.has_exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The child's exit code once it has exited, or `None` while it is still
    /// running. Caches the first observed status: `try_wait` reaps the child once,
    /// so a later poll would otherwise lose the code (contract: the crash message
    /// carries the real exit code).
    pub fn exit_code(&self) -> Option<i32> {
        let mut cached = self.exit_code.lock().unwrap();
        if cached.is_none() {
            if let Ok(Some(status)) = self.child.lock().unwrap().try_wait() {
                *cached = Some(status.exit_code() as i32);
            }
        }
        *cached
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

    /// A harness that copies its stdin to `capture`, byte-for-byte after the
    /// PTY's canonical-mode line discipline (which maps the submitted `\r` to
    /// `\n`). File capture — not `cat >/dev/null` — so tests can assert on the
    /// exact bytes a real TUI would receive.
    fn stdin_capture_spec(capture: &std::path::Path) -> HarnessSpec {
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("cat > \"$1\"")
            .arg("build-stdin-capture")
            .arg(capture.to_string_lossy())
    }

    /// Poll `capture` until its contents contain `needle` (bounded), returning them.
    async fn capture_containing(capture: &std::path::Path, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(contents) = std::fs::read_to_string(capture) {
                if contents.contains(needle) {
                    return contents;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?} in the stdin capture"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn multi_line_prompt_is_framed_as_one_bracketed_paste() {
        // Written raw, a TUI reads every embedded newline as the Enter key and
        // submits the prompt as N fragmented turns; the paste frame makes it
        // one pasted block submitted once.
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let session = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();

        session
            .write_prompt("do the task\nBuild conversation protocol:\n- rule")
            .unwrap();

        let captured = capture_containing(&capture, "\u{1b}[201~").await;
        assert_eq!(
            captured, "\u{1b}[200~do the task\nBuild conversation protocol:\n- rule\u{1b}[201~\n",
            "the whole multi-line prompt travels as one paste, submitted once"
        );
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn single_line_prompt_is_written_without_paste_framing() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let session = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();

        session.write_prompt("just ping").unwrap();

        let captured = capture_containing(&capture, "just ping").await;
        assert_eq!(
            captured, "just ping\n",
            "a single-line prompt needs no paste frame"
        );
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn embedded_paste_terminator_cannot_end_the_paste_early() {
        // A prompt that smuggles the paste-end marker (thread content is
        // reviewer-supplied) would otherwise close the frame mid-prompt and
        // replay the rest as raw keystrokes — newlines as Enter included.
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let session = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();

        session
            .write_prompt("review this\u{1b}[201~\nrm -rf /tmp/pwned")
            .unwrap();

        let captured = capture_containing(&capture, "pwned").await;
        assert_eq!(
            captured.matches("\u{1b}[201~").count(),
            1,
            "only the framing terminator survives: {captured:?}"
        );
        assert!(
            captured.ends_with("\u{1b}[201~\n"),
            "the frame closes at the end, not mid-prompt: {captured:?}"
        );
        session.kill_and_reap();
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
    async fn child_inherits_the_daemon_environment() {
        // Without inheritance the child gets an empty env and portable-pty falls
        // back to the confstr PATH ("/usr/bin:/bin:/usr/sbin:/sbin"), so a harness
        // installed anywhere else (`claude` under ~/.local/bin, a toolbox shim)
        // fails to spawn at all.
        let parent_path = std::env::var("PATH").unwrap();
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'PATH[%s]' \"$PATH\"");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();

        let out = read_until(&mut rx, "PATH[").await;
        assert!(
            out.contains(&format!("PATH[{parent_path}")),
            "child PATH should be the daemon's; got: {out:?}"
        );
    }

    #[tokio::test]
    async fn resolves_the_binary_through_the_daemon_path() {
        // portable-pty resolves argv[0] against the *builder's* PATH, not the
        // process's; with none set it falls back to confstr `_CS_PATH`
        // ("/usr/bin:/bin:/usr/sbin:/sbin"). That is why `sh` spawned fine but
        // `claude` — installed under ~/.local/bin, a toolbox shim, mise, nvm —
        // died with "No viable candidates found in PATH".
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("build-test-harness");
        std::fs::write(&bin, "#!/bin/sh\nprintf 'HARNESS_OK'\n").unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // Only ever *append* to PATH so concurrent tests stay unaffected.
        let path = format!(
            "{}:{}",
            std::env::var("PATH").unwrap(),
            dir.path().display()
        );
        std::env::set_var("PATH", &path);

        let spec = HarnessSpec::new("build-test-harness");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();

        let out = read_until(&mut rx, "HARNESS_OK").await;
        assert!(out.contains("HARNESS_OK"), "got: {out:?}");
    }

    #[tokio::test]
    async fn missing_harness_names_the_binary_and_the_path() {
        let spec = HarnessSpec::new("build-definitely-missing-harness");
        let msg = match PtySession::spawn(&spec, None, small_pty()) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("a missing harness must not spawn"),
        };
        assert!(
            msg.contains("build-definitely-missing-harness")
                && msg.contains(&std::env::var("PATH").unwrap()),
            "error should name the harness and the PATH searched; got: {msg}"
        );
    }

    #[tokio::test]
    async fn spec_env_overrides_the_inherited_value() {
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'TERM[%s]' \"$TERM\"")
            .env("TERM", "build-test-term");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();

        let out = read_until(&mut rx, "TERM[").await;
        assert!(out.contains("TERM[build-test-term]"), "got: {out:?}");
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
    async fn exit_code_is_cached_and_survives_repeated_polls() {
        // A harness that exits with a distinct non-zero code — the crash case whose
        // code the attention message must carry.
        let spec = HarnessSpec::new("sh").arg("-c").arg("exit 3");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let mut code = None;
        for _ in 0..50 {
            if let Some(c) = session.exit_code() {
                code = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(code, Some(3), "the real exit code is observed");
        // Repeated polls keep returning it even though try_wait reaps only once.
        assert_eq!(session.exit_code(), Some(3));
        assert!(session.has_exited());
    }

    #[tokio::test]
    async fn exited_within_bridges_the_gap_until_the_exit_is_reapable() {
        // The prompt-write race, with the reap lag under our control: a child
        // that is still un-reapable right now but exits shortly after models
        // the kernel window where the PTY already returned EIO while
        // `try_wait` still says "running".
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 0.15");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        assert!(
            !session.has_exited(),
            "precondition: the exit status must not be reapable yet"
        );
        assert!(
            session.exited_within(Duration::from_millis(500)),
            "the bounded wait must observe the exit that a single poll misses"
        );
    }

    #[tokio::test]
    async fn exited_within_gives_up_on_a_harness_that_keeps_running() {
        // A genuinely live harness must not be misread as exited — and the
        // wait must actually be bounded, not hang.
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let waited = Instant::now();
        assert!(!session.exited_within(Duration::from_millis(100)));
        assert!(
            waited.elapsed() < Duration::from_secs(2),
            "the wait must return promptly after its deadline"
        );
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn ready_within_returns_once_the_child_enables_paste_mode() {
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf '\\033[?2004h'; sleep 5");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let waited = Instant::now();
        assert!(session.ready_within(Duration::from_secs(2)));
        assert!(
            waited.elapsed() < Duration::from_secs(1),
            "readiness must be observed promptly, not at the deadline"
        );
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn ready_within_ignores_output_that_is_not_the_paste_mode_signal() {
        // The defect this guards: a harness whose FIRST output is a modal
        // workspace-trust dialog. Treating any byte as readiness put the prompt
        // into that dialog and let the submit key answer it, so the agent
        // received nothing at all.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'Do you trust the files in this folder?'; sleep 5");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        assert!(!session.ready_within(Duration::from_millis(300)));
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn ready_within_sees_a_paste_mode_signal_split_across_reads() {
        // The sequence is 8 bytes and a TUI can flush mid-escape; a scan that
        // only looked inside one chunk would miss it and burn the full grace.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf '\\033[?20'; sleep 0.2; printf '04h'; sleep 5");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        assert!(session.ready_within(Duration::from_secs(3)));
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn ready_within_gives_up_bounded_when_the_child_stays_silent() {
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let waited = Instant::now();
        assert!(!session.ready_within(Duration::from_millis(100)));
        assert!(
            waited.elapsed() < Duration::from_secs(2),
            "the wait must return promptly after its deadline"
        );
        session.kill_and_reap();
    }

    #[tokio::test]
    async fn ready_within_stops_waiting_for_a_child_that_exits_silently() {
        // An instantly dead harness will never become ready; the wait must not
        // burn its full deadline before the exit-race guard downstream can run.
        let spec = HarnessSpec::new("sh").arg("-c").arg("exit 0");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let waited = Instant::now();
        session.ready_within(Duration::from_secs(10));
        assert!(
            waited.elapsed() < Duration::from_secs(5),
            "a dead child must end the wait early, not at the deadline"
        );
    }

    #[test]
    fn marker_stripping_cannot_recombine_split_markers() {
        // A single removal pass would splice the surrounding bytes of these
        // nested payloads into fresh markers — the classic sanitizer bypass.
        let sneaky = "\u{1b}[200\u{1b}[200~~ payload \u{1b}[20\u{1b}[201~1~";
        let sanitized = strip_bracketed_paste_markers(sneaky);
        assert!(
            !sanitized.contains("\u{1b}[200~") && !sanitized.contains("\u{1b}[201~"),
            "no marker may survive or re-form: {sanitized:?}"
        );
    }

    #[test]
    fn marker_stripping_stays_linear_on_deeply_nested_markers() {
        // Prompt text carries reviewer-supplied thread content, and the whole
        // dispatch runs under the app-wide lock — so a sanitizer whose cost
        // grows with nesting depth is a remote stall of every project, not a
        // slow function. `nest` is the adversarial shape: each pass peels one
        // level, so a rescan-the-whole-string loop is quadratic.
        // The core is a real marker; each wrap is inert until the level inside
        // it is removed, at which point the surrounding bytes splice into a
        // fresh marker. So the payload peels exactly one level per pass.
        let mut nested = String::from(PASTE_END);
        for _ in 0..8_000 {
            nested = format!("\u{1b}[20{nested}0~");
        }
        let started = std::time::Instant::now();
        let sanitized = strip_bracketed_paste_markers(&nested);
        let elapsed = started.elapsed();

        assert!(
            !sanitized.contains(PASTE_START) && !sanitized.contains(PASTE_END),
            "no marker may survive or re-form"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "sanitizing {} bytes took {elapsed:?}; a nesting-sensitive scan stalls the bridge",
            nested.len()
        );
    }

    #[tokio::test]
    async fn subscribers_observe_closed_when_the_child_exits() {
        // The keyed-terminal pump contract (worktree surfaces §2.4): a shell
        // exiting on its own must end every subscription with `Closed`, even
        // though the PtySession itself is still held in a map.
        let spec = HarnessSpec::new("sh").arg("-c").arg("printf hi");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();

        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "EOF must close the broadcast");

        // A subscription taken after EOF is closed from the start.
        let mut late = session.subscribe();
        assert!(matches!(
            late.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
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
