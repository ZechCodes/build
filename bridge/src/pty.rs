//! Full-PTY harness sessions — the subprocess implementation of
//! [`AgentSession`](crate::harness::AgentSession), and the only one that also
//! offers the [`TerminalView`](crate::harness::TerminalView) capability.
//!
//! There is no harness SDK behind this one. Dispatching a phase means writing a
//! prompt into the agent's PTY; the user dropping in means attaching to the
//! same PTY. Output is broadcast to every subscriber (the relay stream, the
//! quiescence monitor) and the time of the last byte is what "working" is
//! measured from — a terminal has no other way to tell.
//!
//! What stays in this module is the part that is genuinely a terminal's: PATH
//! resolution for the binary, bracketed-paste framing, and the paint-settling
//! that decides when a TUI will accept a turn. Every session question with a
//! transport-free answer is asked through the trait instead, so nothing above
//! here has to know any of it.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

use crate::harness::{AgentSession, AgentStatus, HarnessError, TerminalView, Turn};
use crate::priority::{ChildKind, ChildPlacement, Launch};

/// Resolve `binary` the way a shell would, against the daemon's PATH (or the
/// spec's own override). We do this rather than leaving it to portable-pty:
/// portable-pty searches the *CommandBuilder's* environment, which falls back to
/// confstr `_CS_PATH` ("/usr/bin:/bin:/usr/sbin:/sbin") and produces an opaque
/// "No viable candidates" error that names neither the harness nor the fix.
pub(crate) fn resolve_binary(spec: &HarnessSpec) -> Result<PathBuf, HarnessError> {
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
    Err(HarnessError::NotFound {
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

/// How recently an agent's PTY must have painted for it to count as WORKING.
///
/// Aliveness alone is the wrong signal: an agent tab opened yesterday and left
/// at its prompt is alive and doing nothing, and a rail that pulses at it
/// forever teaches you to ignore the pulse. A working agent paints — spinners,
/// tool output, tokens — so silence means it is waiting for you, which is the
/// state the dot must NOT claim is progress.
///
/// This is the terminal's guess, and it lives here because it is the terminal's:
/// a harness that reports its own turn boundaries answers
/// [`AgentSession::status`] from those instead.
pub const AGENT_WORKING_WINDOW: Duration = Duration::from_secs(30);

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
    /// How long the submit key must trail the prompt text.
    ///
    /// Zero means text and submit travel in one write — right for scripted
    /// harnesses reading a plain pipe. A real TUI needs the gap: paste frame
    /// and Enter written together arrive in ONE stdin read, and claude's
    /// editor handles the Enter before the paste has committed to its composer
    /// — the prompt sits there pasted but unsubmitted. The delayed submit is
    /// written from a detached thread so no caller (some hold the app-wide
    /// state lock through a delivery) ever sleeps for it.
    pub submit_delay: Duration,
    pub known_session_id: Option<String>,
    pub compaction_sidecar: Option<PathBuf>,
    /// The Build agent this spec starts, when the harness logs anything that
    /// must name it (task #58: a usage limit in bridge.log says whose turn).
    pub agent_id: Option<String>,
    /// An agent's harness or the user's own shell: where it runs, and behind
    /// whom (`crate::priority`).
    pub kind: ChildKind,
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
            submit_delay: Duration::ZERO,
            known_session_id: None,
            compaction_sidecar: None,
            agent_id: None,
            kind: ChildKind::Agent,
        }
    }

    /// This spec starts the user's own shell, not an agent.
    pub fn as_terminal(mut self) -> Self {
        self.kind = ChildKind::Terminal;
        self
    }

    /// Name the Build agent this spec starts, for the harness's own log lines.
    pub fn for_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Declare how long this harness's submit key must trail the prompt text.
    pub fn submit_delay(mut self, delay: Duration) -> Self {
        self.submit_delay = delay;
        self
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

    pub fn known_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.known_session_id = Some(session_id.into());
        self
    }

    pub fn compaction_sidecar(mut self, path: PathBuf) -> Self {
        self.compaction_sidecar = Some(path);
        self
    }
}

/// A live agent session bound to a PTY. Cloneable handles share one underlying PTY.
pub struct PtySession {
    /// Shared, not owned: a delayed submit key is written from a detached
    /// thread that outlives the borrow a caller holds on the session.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
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
    submit_delay: Duration,
    /// The child's exit code, cached the first time it is observed. `try_wait`
    /// reaps the child exactly once, so the status must be remembered here or the
    /// crash-detection message ("exit code N") could never recover the code after
    /// the first poll.
    exit_code: Mutex<Option<i32>>,
    /// A launch-known or provider-located conversation identity. `None` is for
    /// a terminal with no conversation to name, such as the human's shell.
    identity: Option<crate::harness::SessionIdentitySource>,
    activity_rx: Mutex<Option<broadcast::Receiver<crate::harness::ActivityReport>>>,
    compaction_sidecar: Option<PathBuf>,
}

/// Start `binary` with `spec`'s arguments and environment on `slave`, at a
/// gate, and place it as `placement` says before it runs (`crate::priority`):
/// the one spawn there is, whatever the placement comes to.
fn spawn_child(
    slave: &dyn portable_pty::SlavePty,
    spec: &HarnessSpec,
    binary: &std::path::Path,
    cwd: Option<&std::path::Path>,
    placement: &ChildPlacement,
) -> Result<Box<dyn Child + Send + Sync>, HarnessError> {
    let launch = Launch::gated(binary, &spec.args)
        .map_err(|error| HarnessError::Session(error.to_string()))?;
    let mut cmd = CommandBuilder::new(&launch.program);
    cmd.args(&launch.args);
    for key in &spec.unset {
        cmd.env_remove(key);
    }
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.cwd(cwd);
    }
    let mut child = slave
        .spawn_command(cmd)
        .map_err(|e| HarnessError::Session(e.to_string()))?;
    let Some(pid) = child.process_id() else {
        return Err(HarnessError::Session("the spawned child has no pid".into()));
    };
    let placed = placement.place(launch.gate, spec.kind, pid, &mut || {
        matches!(child.try_wait(), Ok(None))
    });
    match placed.released {
        Ok(()) => Ok(child),
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(HarnessError::Session(format!(
                "the child never ran: {reason}"
            )))
        }
    }
}

impl PtySession {
    /// Spawn `spec` in a fresh PTY of `size`, optionally in `cwd` (the worktree),
    /// placed the way the daemon places every child (`crate::priority`).
    pub fn spawn(
        spec: &HarnessSpec,
        cwd: Option<PathBuf>,
        size: PtySize,
    ) -> Result<PtySession, HarnessError> {
        Self::spawn_placed(spec, cwd, size, ChildPlacement::current())
    }

    /// [`spawn`](Self::spawn) under an explicit placement — the seam the tests
    /// use to prove the scoped path and its fallback without touching the
    /// process-wide choice.
    pub fn spawn_placed(
        spec: &HarnessSpec,
        cwd: Option<PathBuf>,
        size: PtySize,
        placement: ChildPlacement,
    ) -> Result<PtySession, HarnessError> {
        if let Some(path) = spec.compaction_sidecar.as_ref() {
            let parent = path.parent().ok_or_else(|| {
                HarnessError::Setup(format!(
                    "compaction sidecar {} has no parent",
                    path.display()
                ))
            })?;
            std::fs::create_dir_all(parent).map_err(|error| {
                HarnessError::Setup(format!("create {}: {error}", parent.display()))
            })?;
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .map_err(|error| {
                    HarnessError::Setup(format!("create {}: {error}", path.display()))
                })?;
        }
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| HarnessError::Session(e.to_string()))?;

        let binary = resolve_binary(spec)?;
        let child = spawn_child(&*pair.slave, spec, &binary, cwd.as_deref(), &placement)?;
        // Close the slave in the parent so EOF propagates when the child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| HarnessError::Session(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| HarnessError::Session(e.to_string()))?;

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
            writer: Arc::new(Mutex::new(writer)),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            output_tx,
            last_activity,
            accepts_paste,
            submit: spec.submit.clone(),
            settle: spec.settle,
            submit_delay: spec.submit_delay,
            exit_code: Mutex::new(None),
            identity: None,
            activity_rx: Mutex::new(None),
            compaction_sidecar: spec.compaction_sidecar.clone(),
        })
    }

    /// Hand this session its launch-known or transcript-located conversation
    /// identity.
    ///
    /// Taken after the spawn because a located identity snapshots the transcript
    /// tree before the child exists. A launch-known identity uses the same
    /// carrier without transcript discovery.
    pub fn with_session_identity(
        mut self,
        identity: Option<crate::harness::SessionIdentitySource>,
    ) -> PtySession {
        if let Some(path) = self.compaction_sidecar.clone() {
            *self.activity_rx.lock().unwrap() =
                Some(crate::harness::transcript_activity::follow_sidecar(
                    path,
                    Arc::downgrade(&self.output_tx),
                ));
        }
        self.identity = identity;
        self
    }

    pub fn with_activity_locator(
        self,
        activity_locator: Option<(Box<dyn crate::harness::SessionLocator>, Option<String>)>,
    ) -> PtySession {
        if self.compaction_sidecar.is_none() {
            if let Some((locator, known)) = activity_locator {
                *self.activity_rx.lock().unwrap() =
                    locator.activity(known.as_deref(), Arc::downgrade(&self.output_tx));
            }
        }
        self
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

    /// Kill the harness process.
    pub fn kill(&self) -> Result<(), HarnessError> {
        self.child.lock().unwrap().kill()?;
        Ok(())
    }

    /// Block until the harness exits, returning whether it exited successfully.
    pub fn wait(&self) -> Result<bool, HarnessError> {
        Ok(self.child.lock().unwrap().wait()?.success())
    }

    // The four calls below are the terminal's, and
    // [`TerminalView`](crate::harness::TerminalView) names them. They are
    // inherent so the trait delegates to one body rather than carrying its own,
    // and so a call on a concrete `PtySession` has a single meaning.

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

    /// Write raw bytes to the PTY (user keystrokes from an attached terminal).
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), HarnessError> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    /// Resize the terminal (an attached client changed its viewport).
    pub fn resize(&self, size: PtySize) -> Result<(), HarnessError> {
        self.master
            .lock()
            .unwrap()
            .resize(size)
            .map_err(|e| HarnessError::Session(e.to_string()))
    }

    /// The harness's OS process id, if it is still running.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().unwrap().process_id()
    }

    /// Whether the child exits within `timeout`. `has_exited` is a single
    /// racy poll: a dying harness closes its side of the PTY (so writes fail
    /// with EIO) *before* the OS makes its exit status reapable, so one poll
    /// can report a harness that is already gone as still running. Callers
    /// deciding whether a PTY write error means "crashed" (benign) rather
    /// than "wedged" (fatal) wait out that reap lag here instead.
    pub fn exited_within(&self, timeout: Duration) -> bool {
        Self::exited_within_polling(timeout, || self.has_exited())
    }

    fn exited_within_polling(timeout: Duration, mut has_exited: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if has_exited() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Test-only: age the last-output stamp by `ago`, so a live PTY reports the
    /// silence of one that has been sitting at its prompt for that long.
    ///
    /// The windows this feeds (an agent counts as working for 30 s after it
    /// paints; quiescence demotes after minutes) are far longer than any test
    /// may sleep, and the alternative — a real wait — would make the suite
    /// unrunnable. The process stays genuinely alive; only the clock moves.
    #[cfg(test)]
    pub fn backdate_last_output(&self, ago: Duration) {
        let mut last = self.last_activity.lock().unwrap();
        *last = last.checked_sub(ago).expect("a stamp old enough to age");
    }
}

/// What Build needs from any agent, answered by a subprocess in a full PTY.
///
/// Every answer here is the terminal's: a turn is keystrokes, and the status is
/// synthesized from the age of the last paint. The session is opaque — Build
/// sees what it launched and what the agent reported over MCP, and nothing in
/// between — so it offers the escape hatch.
impl AgentSession for PtySession {
    /// Hand the agent one turn, through the harness's own paste framing and
    /// submit key.
    ///
    /// The readiness check the spec's table names is NOT here: it belongs to
    /// the spawn, which waits out the startup paint once before the first turn
    /// is written. Re-checking per turn would park the caller behind a mid-turn
    /// repaint, and this call must return promptly — see the trait doc.
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.write_prompt(&turn.text)
    }

    /// The four conjuncts of the old `agent_is_working`, minus the two the
    /// session cannot see (the tab's role and whether it is live): exited is
    /// over, painting inside [`AGENT_WORKING_WINDOW`] is working, and silence
    /// past it is waiting for the human.
    ///
    /// A fresh PTY reports `Working` rather than `Starting`, because a spawning
    /// agent is stamped as having just painted and today's rule counts that as
    /// working. Naming the gap is a session protocol's job, not a terminal's.
    fn status(&self) -> AgentStatus {
        match self.exit_code() {
            Some(code) => AgentStatus::Ended { code: Some(code) },
            None if self.idle_for() < AGENT_WORKING_WINDOW => AgentStatus::Working,
            None => AgentStatus::Waiting,
        }
    }

    /// The age of the last byte painted. A terminal has no other evidence that
    /// work is happening, so the paint clock is the whole answer — the same
    /// measurement `idle_for` has always made, under the name the daemon asks
    /// every carrier by.
    fn quiet_for(&self) -> Duration {
        self.idle_for()
    }

    /// Wait out the reap lag — see [`PtySession::exited_within`].
    fn exited_within(&self, timeout: Duration) -> bool {
        PtySession::exited_within(self, timeout)
    }

    /// Kill the harness and reap it — see [`PtySession::kill_and_reap`].
    fn end(&self) {
        self.kill_and_reap();
    }

    fn conversation_artifacts(&self) -> Vec<PathBuf> {
        self.compaction_sidecar.iter().cloned().collect()
    }

    /// Always, for a CLI wrapper: it is opaque, so the human needs a way in.
    fn terminal(&self) -> Option<&dyn TerminalView> {
        Some(self)
    }

    /// Return the launch-known identity immediately or ask the provider locator.
    fn session_id(&self) -> Option<String> {
        match self.identity.as_ref()? {
            crate::harness::SessionIdentitySource::Known(id) => Some(id.clone()),
            crate::harness::SessionIdentitySource::Located(locator) => locator.session_id(),
        }
    }

    fn activity(&self) -> Option<broadcast::Receiver<crate::harness::ActivityReport>> {
        self.activity_rx.lock().unwrap().take()
    }

    /// Test-only: age the paint clock — see
    /// [`PtySession::backdate_last_output`].
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration) {
        PtySession::backdate_last_output(self, ago);
    }
}

/// The escape hatch into a harness Build can only see the outside of.
impl TerminalView for PtySession {
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        PtySession::subscribe(self)
    }

    fn write_input(&self, bytes: &[u8]) -> Result<(), HarnessError> {
        PtySession::write_input(self, bytes)
    }

    fn resize(&self, size: PtySize) -> Result<(), HarnessError> {
        PtySession::resize(self, size)
    }

    fn pid(&self) -> Option<u32> {
        PtySession::pid(self)
    }
}

/// The terminal mechanics behind the PTY's [`AgentSession`] answers.
///
/// Nothing above this module calls any of them: a turn arrives as a value and
/// leaves here as a framed paste, readiness is waited out where the session is
/// opened, and the paint clock is what `quiet_for` reports. They are inherent
/// rather than a second trait because there is one carrier that speaks this
/// vocabulary, and a trait with one implementation is a name standing in front
/// of a body.
impl PtySession {
    /// Write a prompt and submit it (per the harness's `SubmitKey`). A
    /// multi-line prompt bound for an Enter-submitting TUI travels as ONE
    /// bracketed paste: written raw, the TUI would read every embedded newline
    /// as the Enter key and submit the prompt as fragmented turns. Prompts
    /// written verbatim (`SubmitKey::None`) are never framed — that contract
    /// promises the harness the exact bytes.
    pub fn write_prompt(&self, prompt: &str) -> Result<(), HarnessError> {
        let sanitized = strip_bracketed_paste_markers(prompt);
        {
            let mut writer = self.writer.lock().unwrap();
            if self.submit == SubmitKey::Enter && sanitized.contains('\n') {
                writer.write_all(PASTE_START.as_bytes())?;
                writer.write_all(sanitized.as_bytes())?;
                writer.write_all(PASTE_END.as_bytes())?;
            } else {
                writer.write_all(sanitized.as_bytes())?;
            }
            if self.submit_delay.is_zero() {
                writer.write_all(self.submit.bytes())?;
                writer.flush()?;
                return Ok(());
            }
            writer.flush()?;
        }
        // The submit trails the text by the spec's declared delay (see
        // `HarnessSpec::submit_delay`), written off-thread: a turn can be
        // handed over from under the app-wide state lock (the in-place nudge
        // does), and sleeping under it would stall every pump. A failed write
        // here is the child exiting under us — the same race the caller's exit
        // guard already covers for the text write.
        let writer = Arc::clone(&self.writer);
        let delay = self.submit_delay;
        let submit = self.submit.bytes();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let mut writer = writer.lock().unwrap();
            let _ = writer.write_all(submit);
            let _ = writer.flush();
        });
        Ok(())
    }

    /// How long since the PTY last produced output.
    pub fn idle_for(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.last_activity.lock().unwrap())
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

    /// Kill the harness and reap it. `kill` alone leaves a zombie: portable-pty's
    /// unix child does not reap on drop, so every phase transition on a long-lived
    /// daemon would otherwise leak one process-table entry.
    pub fn kill_and_reap(&self) {
        let mut child = self.child.lock().unwrap();
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
    async fn submit_delay_separates_the_enter_from_the_paste() {
        // The coalescing defect this guards: paste frame and Enter written in
        // one burst arrive in one stdin read, and claude's editor handles the
        // Enter before the paste has committed to its composer — the text sits
        // there unsubmitted. The submit must trail the paste by the spec's
        // declared delay.
        // Observed at the capture through canonical mode, which buffers the
        // paste until a line terminator arrives — so the submit's timing IS
        // the timing of anything reaching the file at all.
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let spec = stdin_capture_spec(&capture).submit_delay(Duration::from_millis(300));
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        let written_at = Instant::now();
        session
            .write_prompt("stage two\ndo the next thing")
            .unwrap();
        assert!(
            written_at.elapsed() < Duration::from_millis(200),
            "write_prompt must not sleep out the delay itself — a turn can be \
             handed over from under the app-wide state lock"
        );

        capture_containing(&capture, "\u{1b}[201~\n").await;
        let submitted_after = written_at.elapsed();
        assert!(
            submitted_after >= Duration::from_millis(300),
            "the Enter must trail the paste by the declared delay; it landed after {submitted_after:?}"
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
        // Assert inheritance with a test-private marker instead of depending on
        // the machine's environment.
        let marker = format!("inherit-{}", uuid::Uuid::new_v4());
        std::env::set_var("BUILD_BRIDGE_ENV_INHERITANCE_TEST", &marker);
        // The harness waits for a line before it prints, as
        // `prompt_roundtrips_through_the_pty`'s does. A harness that printed on
        // spawn could be finished before `subscribe` ran — a subscriber only
        // sees what is sent after it, so on a loaded machine this test read an
        // already-closed stream and called an inherited variable missing.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("read _; printf 'MARKER[%s]' \"$BUILD_BRIDGE_ENV_INHERITANCE_TEST\"");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();
        session.write_input(b"go\r").unwrap();

        let out = read_until(&mut rx, "MARKER[").await;
        assert!(
            out.contains(&format!("MARKER[{marker}]")),
            "child should inherit the daemon environment; got: {out:?}"
        );
        std::env::remove_var("BUILD_BRIDGE_ENV_INHERITANCE_TEST");
    }

    #[tokio::test]
    async fn resolves_the_binary_through_the_daemon_path() {
        // portable-pty resolves argv[0] against the *builder's* PATH, not the
        // process's; with none set it falls back to confstr `_CS_PATH`
        // ("/usr/bin:/bin:/usr/sbin:/sbin"). That is why `sh` spawned fine but
        // `claude` — installed under ~/.local/bin, a toolbox shim, mise, nvm —
        // died with "No viable candidates found in PATH".
        // Run PATH variants in separate processes: changing this test runner's
        // environment can race every other harness or Git child it starts.
        const CHILD: &str = "BUILD_PATH_LOOKUP_TEST_CHILD";
        if let Ok(mode) = std::env::var(CHILD) {
            let binary = std::env::var("BUILD_PATH_LOOKUP_TEST_BINARY").unwrap();
            let spec = HarnessSpec::new(&binary);
            if mode == "missing" {
                assert!(matches!(
                    resolve_binary(&spec),
                    Err(HarnessError::NotFound { .. })
                ));
                return;
            }
            let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
            let mut rx = session.subscribe();
            // The marker is produced only after the subscription exists.
            session.write_input(b"go\r").unwrap();
            let out = read_until(&mut rx, "HARNESS_OK").await;
            session.end();
            assert!(out.contains("HARNESS_OK"), "got: {out:?}");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let binary = format!("build-test-harness-{}", uuid::Uuid::new_v4());
        let bin = dir.path().join(&binary);
        std::fs::write(&bin, "#!/bin/sh\nread _; printf 'HARNESS_OK'\n").unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let inherited = std::env::var("PATH").unwrap();
        let with_fixture = format!("{inherited}:{}", dir.path().display());
        // Negative control: the same executable must be unavailable without
        // its directory on the daemon's PATH.
        for (mode, path) in [("present", with_fixture.as_str()), ("missing", &inherited)] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pty::tests::resolves_the_binary_through_the_daemon_path",
                    "--nocapture",
                ])
                .env(CHILD, mode)
                .env("BUILD_PATH_LOOKUP_TEST_BINARY", &binary)
                .env("PATH", path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode} PATH child failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
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
        // Waits for a line for the same reason the test above does: output sent
        // before `subscribe` is output this test can never read.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("read _; printf 'TERM[%s]' \"$TERM\"")
            .env("TERM", "build-test-term");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();
        session.write_input(b"go\r").unwrap();

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
        // The child announces it is running, then waits for our input before
        // it can exit. This pins the first poll before the exit, regardless of
        // how long spawn or the test thread was delayed by the scheduler.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("IFS= read -r arm; printf 'armed\\n'; IFS= read -r release; exit 3");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut output = session.subscribe();
        session.write_input(b"arm\n").unwrap();
        read_until(&mut output, "armed").await;

        assert!(
            !session.has_exited(),
            "precondition: the exit status must not be reapable yet"
        );
        let mut first_poll = true;
        assert!(
            PtySession::exited_within_polling(Duration::from_millis(500), || {
                let exited = session.has_exited();
                if first_poll {
                    assert!(!exited, "the first poll must precede the child's exit");
                    first_poll = false;
                    session.write_input(b"go\n").unwrap();
                }
                exited
            }),
            "the bounded wait must observe the exit that a single poll misses"
        );
        assert!(!first_poll, "the wait polled the child at least once");
        assert_eq!(session.exit_code(), Some(3));
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
        // The budget is calibrated on this machine rather than fixed in
        // milliseconds: the same byte count with no nesting at all is what a
        // linear sanitizer costs here and now, so the comparison absorbs
        // whatever CPU the rest of the suite has left. A fixed budget cannot,
        // and this test used to fail under a full parallel run purely for
        // being descheduled. The added floor keeps a very fast baseline from
        // turning ordinary jitter into a failure.
        let flat = "a".repeat(nested.chars().count());
        let flat_started = std::time::Instant::now();
        let flat_sanitized = strip_bracketed_paste_markers(&flat);
        let linear_cost = flat_started.elapsed();
        assert_eq!(flat_sanitized.len(), flat.len(), "plain text is untouched");

        let started = std::time::Instant::now();
        let sanitized = strip_bracketed_paste_markers(&nested);
        let elapsed = started.elapsed();

        assert!(
            !sanitized.contains(PASTE_START) && !sanitized.contains(PASTE_END),
            "no marker may survive or re-form"
        );
        let budget = linear_cost * 20 + Duration::from_millis(250);
        assert!(
            elapsed < budget,
            "sanitizing {} nested bytes took {elapsed:?} against a {budget:?} budget \
             ({linear_cost:?} for the same bytes unnested); a nesting-sensitive scan \
             stalls the bridge",
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

    /// A terminal names its conversation through the locator the provider gave
    /// it — lazily, so the answer arrives when the harness's own record does,
    /// and by delegation, so there is exactly one place the name comes from.
    #[tokio::test]
    async fn a_pty_answers_the_name_the_locator_it_was_given_finds() {
        struct WhenAsked(Arc<Mutex<Option<String>>>);
        impl crate::harness::SessionLocator for WhenAsked {
            fn session_id(&self) -> Option<String> {
                self.0.lock().unwrap().clone()
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let found = Arc::new(Mutex::new(None));
        let session: Box<dyn AgentSession> = Box::new(
            PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty())
                .unwrap()
                .with_session_identity(Some(crate::harness::SessionIdentitySource::Located(
                    Box::new(WhenAsked(Arc::clone(&found))),
                ))),
        );

        assert_eq!(
            session.session_id(),
            None,
            "nothing the harness could be having has appeared yet"
        );
        *found.lock().unwrap() = Some("sess-on-disk".to_string());
        assert_eq!(session.session_id().as_deref(), Some("sess-on-disk"));
        session.end();
    }

    #[tokio::test]
    async fn a_pty_session_offers_no_surfaces() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let session: Box<dyn AgentSession> =
            Box::new(PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap());

        assert!(session.surfaces().is_none());
        assert!(session.surfaces_changed().is_none());
        session.end();
    }

    #[tokio::test]
    async fn a_pty_session_answers_as_an_agent_session_that_has_a_terminal() {
        // The capability question has one answer, asked in one place: a CLI
        // wrapper is opaque, so it offers the escape hatch. Reached through the
        // trait object the daemon will hold, not the concrete type.
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let spawned = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();
        let session: Box<dyn AgentSession> = Box::new(spawned);

        let terminal = session.terminal().expect("a PTY session has a terminal");
        assert!(
            terminal.pid().is_some(),
            "a live PTY session has a process behind it"
        );
        terminal
            .resize(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        let mut rx = terminal.subscribe();
        terminal.write_input(b"typed by a human\r").unwrap();
        let echoed = read_until(&mut rx, "typed by a human").await;
        assert!(
            echoed.contains("typed by a human"),
            "keystrokes reach the PTY through the capability; got: {echoed:?}"
        );
        session.end();
    }

    /// The PTY does not map "stop this turn" onto ESC bytes, and says so.
    ///
    /// ESC is a keystroke whose meaning belongs to the harness — claude reads
    /// it as stop, another closes a picker with it — and a terminal reports no
    /// turn boundary, so Build could write the bytes and never learn whether
    /// anything stopped. The basement is always accessible, so the refusal
    /// sends the human there rather than pressing it blind. And a refusal is
    /// not a kill: the session is the same session afterwards.
    #[tokio::test]
    async fn a_pty_refuses_to_stop_a_turn_and_names_the_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let spawned = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();
        let session: Box<dyn AgentSession> = Box::new(spawned);

        assert!(!session.can_interrupt(), "a terminal stops no turn");
        let refused = session.interrupt().expect_err("the PTY refuses");
        assert!(
            matches!(&refused, HarnessError::Unsupported(said) if said.contains("terminal")
                && said.contains("Esc")),
            "the refusal says where the thing actually lives: {refused}"
        );
        assert!(
            !matches!(session.status(), AgentStatus::Ended { .. }),
            "and it is a refusal, not a kill — the harness is still running"
        );
        assert!(
            session.session_id().is_none(),
            "a terminal names no conversation: what it resumes is a transcript on disk"
        );
        session.end();
    }

    #[tokio::test]
    async fn send_turn_frames_and_submits_exactly_as_write_prompt_does() {
        // The turn is a value at the interface and keystroke mechanics below
        // it: one bracketed paste, the harness's own submit key, nothing else.
        let dir = tempfile::tempdir().unwrap();
        let capture = dir.path().join("stdin.txt");
        let session = PtySession::spawn(&stdin_capture_spec(&capture), None, small_pty()).unwrap();

        session
            .send_turn(&Turn::new(
                "do the task\nBuild conversation protocol:\n- rule",
            ))
            .unwrap();

        let captured = capture_containing(&capture, "\u{1b}[201~").await;
        assert_eq!(
            captured, "\u{1b}[200~do the task\nBuild conversation protocol:\n- rule\u{1b}[201~\n",
            "a turn travels the way a prompt does — one paste, submitted once"
        );
        session.end();
    }

    #[tokio::test]
    async fn status_is_working_while_the_pty_paints_and_waiting_once_it_stops() {
        // The synthesis is today's 30 s rule and nothing else: an agent that
        // paints is working, one that has been quiet past the window is
        // waiting for the human.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'painting'; sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();
        read_until(&mut rx, "painting").await;

        assert!(
            matches!(session.status(), AgentStatus::Working),
            "a painting agent is working; got {:?}",
            session.status()
        );

        session.backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
        assert!(
            matches!(session.status(), AgentStatus::Waiting),
            "silence past the window is waiting, not progress; got {:?}",
            session.status()
        );
        session.end();
    }

    #[tokio::test]
    async fn status_ends_carrying_the_exit_code() {
        // A dead agent's retained screen is not a heartbeat, and the crash
        // message needs the real code.
        let spec = HarnessSpec::new("sh").arg("-c").arg("exit 3");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        for _ in 0..50 {
            if matches!(session.status(), AgentStatus::Ended { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            matches!(session.status(), AgentStatus::Ended { code: Some(3) }),
            "the exit code survives into the status; got {:?}",
            session.status()
        );
    }

    #[tokio::test]
    async fn working_is_exactly_the_two_conjuncts_it_replaced() {
        // The daemon's pulse reads `status()` now, so this is what keeps the
        // wire field `working` meaning what it always did: in every state a
        // PTY can be in, `Working` must hold precisely when the pair the
        // daemon used to read off the session directly holds.
        let old_rule = |s: &PtySession| !s.has_exited() && s.idle_for() < AGENT_WORKING_WINDOW;
        let agrees = |s: &PtySession, state: &str| {
            assert_eq!(
                matches!(s.status(), AgentStatus::Working),
                old_rule(s),
                "{state}: status disagrees with the rule it replaced; got {:?}",
                s.status()
            );
        };

        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'painting'; sleep 30");
        let live = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = live.subscribe();
        read_until(&mut rx, "painting").await;
        agrees(&live, "painting");

        live.backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
        agrees(&live, "quiet past the window");
        live.end();

        let dead = PtySession::spawn(
            &HarnessSpec::new("sh").arg("-c").arg("exit 3"),
            None,
            small_pty(),
        )
        .unwrap();
        for _ in 0..50 {
            if dead.has_exited() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        agrees(&dead, "exited");
    }

    #[tokio::test]
    async fn quiet_for_is_exactly_the_paint_clock_it_replaced() {
        // The idle sweep and the worktree cards asked `idle_for` directly. They
        // ask `quiet_for` now, and for a PTY the answer must still come off the
        // last byte — the name generalizes, the measurement does not move.
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'painting'; sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();
        let mut rx = session.subscribe();
        read_until(&mut rx, "painting").await;

        let asked: Box<dyn AgentSession> = Box::new(session);
        // Both clocks run, so they are compared with the tolerance of the gap
        // between the two reads rather than for equality.
        let close_enough = |state: &str, session: &dyn AgentSession, painted: Duration| {
            let quiet = session.quiet_for();
            assert!(
                quiet.abs_diff(painted) < Duration::from_millis(250),
                "{state}: quiet_for {quiet:?} is not the paint clock's {painted:?}"
            );
        };
        close_enough("painting", asked.as_ref(), Duration::ZERO);

        asked.backdate_last_output(Duration::from_secs(600));
        close_enough(
            "quiet for ten minutes",
            asked.as_ref(),
            Duration::from_secs(600),
        );
        asked.end();
    }

    #[tokio::test]
    async fn exited_within_waits_out_the_reap_lag_through_the_trait() {
        // `deliver` decides whether a failed turn means "crashed" (benign) or
        // "wedged" (fatal) here, and it asks the trait now. A harness that dies
        // during the grace must still be reported as exited — the single poll
        // this replaces races the kernel's reap.
        let dying: Box<dyn AgentSession> = Box::new(
            PtySession::spawn(
                &HarnessSpec::new("sh").arg("-c").arg("sleep 0.2; exit 3"),
                None,
                small_pty(),
            )
            .unwrap(),
        );
        assert!(
            dying.exited_within(Duration::from_secs(5)),
            "a harness that dies inside the grace is a crash, not a wedge"
        );
        assert_eq!(dying.status(), AgentStatus::Ended { code: Some(3) });

        let live: Box<dyn AgentSession> = Box::new(
            PtySession::spawn(
                &HarnessSpec::new("sh").arg("-c").arg("sleep 30"),
                None,
                small_pty(),
            )
            .unwrap(),
        );
        assert!(
            !live.exited_within(Duration::from_millis(50)),
            "a session that outlives the grace is wedged, and its error is real"
        );
        live.end();
    }

    #[tokio::test]
    async fn a_pty_leaves_no_epitaph_of_its_own() {
        // Last words are the terminal's, and a terminal's screen belongs to the
        // tab: the idle sweep reads `tab.screen` for a PTY exactly as it always
        // has. Answering here too would be a second copy of one crash, free to
        // disagree with the first.
        let session: Box<dyn AgentSession> = Box::new(
            PtySession::spawn(
                &HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("printf 'out of quota'; exit 3"),
                None,
                small_pty(),
            )
            .unwrap(),
        );
        for _ in 0..50 {
            if matches!(session.status(), AgentStatus::Ended { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(session.epitaph(), None);
    }

    #[tokio::test]
    async fn ending_a_session_reaps_the_harness() {
        // `end` is the whole lifecycle call the daemon gets, so it must reap:
        // killing without reaping leaks a zombie per session on a daemon that
        // never restarts.
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 30");
        let session = PtySession::spawn(&spec, None, small_pty()).unwrap();

        session.end();

        assert!(
            session.has_exited(),
            "an ended session is reaped, not merely signalled"
        );
    }

    /// A spec that captures its nice at startup, waits for the test to
    /// subscribe, then reports that captured value. The child is lowered at
    /// its gate before it runs a thing, so its first fork sees the nice.
    fn nice_reporting_spec() -> HarnessSpec {
        let mut spec = HarnessSpec::new("sh");
        spec.args = vec![
            "-c".into(),
            "started_nice=$(nice); IFS= read -r arm; printf 'NICE=%s\\n' \"$started_nice\"; IFS= read -r hold"
                .into(),
        ];
        spec
    }

    struct EndNiceSession(PtySession);

    impl Drop for EndNiceSession {
        fn drop(&mut self) {
            self.0.end();
        }
    }

    fn nice_from_complete_record(seen: &str) -> Option<String> {
        let after = seen.split_once("NICE=")?.1;
        let line = after.split_once('\n')?.0.trim_end_matches('\r');
        (!line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| line.to_string())
    }

    async fn nice_report(session: &PtySession) -> (String, String) {
        let mut output = session.subscribe();
        session.write_input(b"report\n").unwrap();
        let mut seen = String::new();
        let nice = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match output.recv().await {
                    Ok(chunk) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(error) => panic!("nice report stream ended: {error}; got {seen:?}"),
                }
                if let Some(nice) = nice_from_complete_record(&seen) {
                    return nice;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for a complete NICE record; got {seen:?}"));
        (seen, nice)
    }

    async fn reported_nice(session: &PtySession) -> String {
        nice_report(session).await.1
    }

    #[test]
    fn nice_report_waits_for_a_complete_numeric_record() {
        let mut seen = "NICE=".to_string();
        assert_eq!(nice_from_complete_record(&seen), None);
        seen.push('1');
        assert_eq!(nice_from_complete_record(&seen), None);
        seen.push('9');
        assert_eq!(nice_from_complete_record(&seen), None);
        seen.push_str("\r\n");
        assert_eq!(nice_from_complete_record(&seen), Some("19".to_string()));
        assert_eq!(nice_from_complete_record("NICE=wrong\r\n"), None);
    }

    /// A stand-in for `busctl` that refuses every scope, and a placement that
    /// uses it with deadlines a test can wait out.
    fn refusing_busctl(dir: &std::path::Path) -> ChildPlacement {
        let path = dir.join("busctl");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'Failed to connect to bus: No such file or directory' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        ChildPlacement::TransientScope {
            busctl: path,
            systemctl: None,
            bound_to: None,
            deadlines: crate::priority::ScopeDeadlines {
                call: Duration::from_millis(500),
                arrival: Duration::from_millis(200),
            },
        }
    }

    /// The one thing every agent child has in common, whatever else its
    /// placement does: it runs behind the daemon. Read back through the PTY
    /// itself, so the nice is proven on the process the terminal is attached to.
    /// What an agent child should report: ten below this process, which the
    /// test runner may itself run niced (the gates run under `nice -n 10`).
    fn agent_nice() -> String {
        crate::priority::child_nice_for(crate::priority::own_nice(), crate::priority::CHILD_NICE)
            .to_string()
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_spawned_agent_runs_ten_below_the_daemon() {
        let session =
            EndNiceSession(PtySession::spawn(&nice_reporting_spec(), None, small_pty()).unwrap());
        assert_eq!(reported_nice(&session.0).await, agent_nice());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_user_s_shell_without_a_scope_is_half_a_step_down() {
        // With no scope to rank it in (the tests never install one), the
        // user's shell runs half a step down: below the daemon, above the
        // agents.
        let spec = nice_reporting_spec().as_terminal();
        let session = EndNiceSession(PtySession::spawn(&spec, None, small_pty()).unwrap());
        assert_eq!(
            reported_nice(&session.0).await,
            crate::priority::child_nice_for(
                crate::priority::own_nice(),
                crate::priority::TERMINAL_NICE_WITHOUT_SCOPE
            )
            .to_string()
        );
    }

    /// The fallback, seen from the terminal: a manager that refuses the scope
    /// costs the child nothing but the scope. It runs once, niced, and the
    /// terminal shows the child's own output and not a word of the refusal,
    /// which went to the daemon's log.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_refused_scope_is_silent_on_the_terminal_and_the_child_runs_niced() {
        let dir = tempfile::tempdir().unwrap();
        let placement = refusing_busctl(dir.path());
        let session = EndNiceSession(
            PtySession::spawn_placed(&nice_reporting_spec(), None, small_pty(), placement).unwrap(),
        );
        let (seen, nice) = nice_report(&session.0).await;
        assert!(
            !seen.contains("Failed") && !seen.contains("bus"),
            "the refusal reached the terminal: {seen:?}"
        );
        assert_eq!(nice, agent_nice());
    }
}
