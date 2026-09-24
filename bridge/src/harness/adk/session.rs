use super::activity::ActivitySlot;
use super::protocol::{publish_status, PendingInterrupt, ProtocolState, ACTIVITY_BACKLOG};
use super::reader::ProtocolReader;
use crate::harness::surfaces::{AgentSurfaces, SurfaceRevision};
use crate::harness::{
    ActivityReport, AgentSession, AgentStatus, HarnessError, SessionStatusSnapshot, Turn,
    TurnChoiceSupport,
};
use crate::models::{AgentProvider, ModelChoice};
use crate::priority::ChildPlacement;
use crate::pty::HarnessSpec;
use serde_json::json;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

pub struct AdkSession {
    /// Shared with the reader and the startup watchdog, the two things
    /// besides Build that may end this child: a model it was not asked for,
    /// and a silence that outlasts the deadline.
    child: Arc<Mutex<Child>>,
    /// `None` once the session has ended — the pipe is dropped so the child
    /// sees EOF and can leave on its own terms before it is killed.
    stdin: Mutex<Option<ChildStdin>>,
    state: Arc<Mutex<ProtocolState>>,
    status_updates: watch::Sender<SessionStatusSnapshot>,
    activity: ActivitySlot,
    revision: SurfaceRevision,
    /// The child's exit code, cached the first time it is observed: the status
    /// can be collected exactly once, and the crash message is written from it
    /// long after.
    exit_code: Mutex<Option<i32>>,
    #[cfg(test)]
    delimiter_blocked: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

struct RestoreFdFlags {
    fd: RawFd,
    flags: libc::c_int,
}

impl Drop for RestoreFdFlags {
    fn drop(&mut self) {
        loop {
            if unsafe { libc::fcntl(self.fd, libc::F_SETFL, self.flags) } >= 0
                || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                break;
            }
        }
    }
}

fn wait_writable(fd: RawFd) -> io::Result<()> {
    let mut readiness = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    loop {
        let ready = unsafe { libc::poll(&mut readiness, 1, -1) };
        if ready > 0 {
            return Ok(());
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl AdkSession {
    /// Spawn `spec` with piped stdio — no PTY — rooted at `cwd`, and start
    /// reading its protocol. Hands back the session and its activity, already
    /// subscribed.
    ///
    /// The stream is subscribed here rather than by the caller for the reason
    /// the PTY's is: the child starts talking the moment it is forked, and an
    /// event minted before anyone subscribed is an event nobody sees.
    ///
    /// There is no readiness dance: a session protocol takes a turn as a value,
    /// so the only thing a caller waits for is the child's own `init` line,
    /// which [`status`](AgentSession::status) reports as `Starting` until it
    /// arrives.
    pub fn spawn(
        spec: &HarnessSpec,
        cwd: Option<PathBuf>,
        choice: &ModelChoice,
    ) -> Result<(AdkSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        AdkSession::spawn_with_startup_deadline(
            spec,
            cwd,
            choice,
            crate::orchestrator::HARNESS_READY_GRACE,
        )
    }

    /// [`spawn`](AdkSession::spawn) with the startup deadline chosen by the
    /// caller — how a test ends a child that never speaks without waiting out
    /// the real grace.
    ///
    /// The deadline is how long the child may go without announcing itself
    /// AFTER the first turn is written to it: the CLI emits no `init` line
    /// until it has read a turn, so a session nobody has spoken to yet is
    /// `Starting` for as long as it likes. A child still silent when it
    /// expires is ended with that as its last words, instead of holding
    /// `Starting` until the idle sweep explains the silence as nothing —
    /// the codex carrier's reconciliation timeout, in this protocol's shape.
    pub fn spawn_with_startup_deadline(
        spec: &HarnessSpec,
        cwd: Option<PathBuf>,
        choice: &ModelChoice,
        startup_deadline: Duration,
    ) -> Result<(AdkSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        let binary = crate::pty::resolve_binary(spec)?;
        // Placed the way every child of the daemon is (`crate::priority`):
        // in a scope of its own behind the daemon and the user's apps where
        // the user's systemd answers, niced everywhere, before it runs a
        // thing — the same gate a terminal's child goes through.
        let mut child =
            ChildPlacement::current().spawn_command(spec.kind, &binary, &spec.args, |command| {
                for key in &spec.unset {
                    command.env_remove(key);
                }
                for (key, value) in &spec.env {
                    command.env(key, value);
                }
                if let Some(cwd) = &cwd {
                    command.current_dir(cwd);
                }
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            })?;

        let state = Arc::new(Mutex::new(ProtocolState::new(choice)));
        state.lock().unwrap().agent_id = spec.agent_id.clone();
        let (sender, subscribed) = broadcast::channel(ACTIVITY_BACKLOG);
        let activity: ActivitySlot = Arc::new(Mutex::new(Some(sender)));
        let revision = SurfaceRevision::default();
        let (status_updates, _) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin = child.stdin.take();
        let child = Arc::new(Mutex::new(child));

        if let Some(stdout) = stdout {
            let mut reader = ProtocolReader::new(
                Arc::clone(&state),
                Arc::clone(&activity),
                revision.clone(),
                status_updates.clone(),
            )
            .ending(Arc::clone(&child));
            let slot = Arc::clone(&activity);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) => reader.read_line(&line),
                        Err(_) => break,
                    }
                }
                reader.end_stream();
                reader.publish_status(AgentStatus::Ended { code: None });
                // The child's account of itself is over: drop the sender so
                // every subscriber sees `Closed` and the pump performs the
                // death rites.
                slot.lock().unwrap().take();
            });
        }

        // Stderr is read on its own thread rather than left in the pipe: a
        // harness that writes more than a pipe buffer's worth of warnings would
        // otherwise block forever mid-turn, and the last line of it is the
        // epitaph of a child that dies before it can report a result.
        if let Some(stderr) = stderr {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        state.lock().unwrap().last_stderr_line = Some(line);
                    }
                }
            });
        }

        start_startup_watchdog(Arc::clone(&state), Arc::clone(&child), startup_deadline);

        Ok((
            AdkSession {
                child,
                stdin: Mutex::new(stdin),
                state,
                status_updates,
                activity,
                revision,
                exit_code: Mutex::new(None),
                #[cfg(test)]
                delimiter_blocked: Mutex::new(None),
            },
            subscribed,
        ))
    }

    /// Write one protocol line to the child's stdin, and return.
    ///
    /// The whole of what this session says to its child — a turn, an interrupt
    /// — is one line on the same pipe, and both callers return on the write for
    /// the same reason: the daemon speaks to a session from under the app-wide
    /// state lock.
    fn write_line(&self, line: &str) -> Result<(), HarnessError> {
        let mut stdin = self.stdin.lock().unwrap();
        let pipe = stdin.as_mut().ok_or_else(|| {
            HarnessError::Session("this session has ended — it takes no more turns".to_string())
        })?;
        pipe.write_all(line.as_bytes())?;
        pipe.write_all(b"\n")?;
        pipe.flush()?;
        Ok(())
    }

    /// Commit the turn at its final newline. The child cannot answer an
    /// incomplete line, and the reader cannot handle its answer until the
    /// state update is done. Neither a large payload nor a full pipe may hold
    /// the state lock while waiting for the child to read.
    fn write_turn_line(&self, line: &str) -> Result<(), HarnessError> {
        let mut stdin = self.stdin.lock().unwrap();
        let pipe = stdin.as_mut().ok_or_else(|| {
            HarnessError::Session("this session has ended — it takes no more turns".to_string())
        })?;
        pipe.write_all(line.as_bytes())?;
        let fd = pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let _restore_flags = RestoreFdFlags { fd, flags };
        let committed = loop {
            let mut state = self.state.lock().unwrap();
            match pipe.write(b"\n") {
                Ok(1) => {
                    if !state.turn_open {
                        state.turn_had_success = false;
                    }
                    state.turn_open = true;
                    state.first_turn_at.get_or_insert_with(Instant::now);
                    // A queued steering turn inherits the interrupt's result boundary.
                    if let Some(pending) = state.pending_interrupt.as_mut() {
                        pending.steered = true;
                    }
                    publish_status(&self.status_updates, state.live_status());
                    break Ok(());
                }
                Ok(_) => break Err(io::Error::from(io::ErrorKind::WriteZero).into()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    drop(state);
                    #[cfg(test)]
                    if let Some(signal) = self.delimiter_blocked.lock().unwrap().take() {
                        let _ = signal.send(());
                    }
                    wait_writable(fd)?;
                }
                Err(error) => break Err(error.into()),
            }
        };
        committed
    }

    /// Ask the child to run `model` from the next turn on, if it is not what
    /// was last asked for. One `set_model` control request, recorded before
    /// the write for the reason the interrupt is; what the child answers is
    /// read later by the reader thread.
    fn apply_model(&self, model: Option<&str>) -> Result<(), HarnessError> {
        let Some(model) = model else {
            return Ok(());
        };
        let request_id = {
            let mut state = self.state.lock().unwrap();
            if state.requested_model.as_deref() == Some(model) {
                return Ok(());
            }
            let request_id = uuid::Uuid::new_v4().to_string();
            state.requested_model = Some(model.to_string());
            state
                .pending_model_changes
                .insert(request_id.clone(), model.to_string());
            request_id
        };
        let line = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "set_model", "model": model },
        })
        .to_string();
        if let Err(refused) = self.write_line(&line) {
            self.state
                .lock()
                .unwrap()
                .pending_model_changes
                .remove(&request_id);
            return Err(refused);
        }
        Ok(())
    }

    /// The child's exit code once it has exited, cached on first sight.
    ///
    /// `try_wait` reaps the child exactly once, so the status has to be
    /// remembered here or a later poll would report a running session.
    fn exit_code(&self) -> Option<i32> {
        let mut cached = self.exit_code.lock().unwrap();
        if cached.is_none() {
            if let Ok(Some(status)) = self.child.lock().unwrap().try_wait() {
                *cached = Some(observed_code(status));
            }
        }
        let code = *cached;
        drop(cached);
        if let Some(code) = code {
            publish_status(
                &self.status_updates,
                AgentStatus::Ended { code: Some(code) },
            );
        }
        code
    }
}

/// Watch for a child that was handed a turn and never announced itself.
///
/// Polls rather than waits on the child, because reaping is the session's
/// business alone (`try_wait` collects the status exactly once, and a watchdog
/// that took it would leave the session reporting a running child forever).
/// Ending is a `kill` and nothing more: the reader sees the stream close and
/// performs the death rites, and the session reaps on its next poll. The
/// thread leaves as soon as the child announces itself, the session is
/// ended, or the deadline has been spent.
fn start_startup_watchdog(
    state: Arc<Mutex<ProtocolState>>,
    child: Arc<Mutex<Child>>,
    deadline: Duration,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(20));
        let expired = {
            let mut state = state.lock().unwrap();
            if state.announced || state.closed {
                return;
            }
            let Some(first_turn_at) = state.first_turn_at else {
                continue;
            };
            if first_turn_at.elapsed() < deadline {
                continue;
            }
            state.reported_error = Some(format!(
                "claude did not announce itself within {deadline:?} of its first turn"
            ));
            state.closed = true;
            true
        };
        if expired {
            let _ = child.lock().unwrap().kill();
            return;
        }
    });
}

/// The code a child exited with. A child killed by a signal has no code of its
/// own, so it is reported the way a shell reports one — `128 + signal` — rather
/// than as the `None` that means "no process behind this session at all".
fn observed_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

impl AgentSession for AdkSession {
    /// Write the turn as one `user` line and return.
    ///
    /// No framing, no submit key, no delay: the protocol takes a turn as a
    /// value. The turn counts as accepted the moment the write lands, which is
    /// what starts the `Working` window — a failed write starts nothing, so the
    /// caller's crashed-versus-wedged check reads a session that never began
    /// the turn.
    ///
    /// A frozen choice is applied first, the way the codex carrier applies
    /// one on its `turn/start`: a model the child is not yet running is
    /// asked for with a `set_model` control request written ahead of the
    /// turn, on the same pipe, so the turn runs on it. A choice this child
    /// cannot take in place — another provider, another effort, a model
    /// cleared back to the default — is refused with the sentence that says
    /// where it lives, never dropped on the floor: a turn that silently ran
    /// on the wrong settings would report itself as the settings the human
    /// chose.
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        if let Some(frozen) = &turn.choice {
            frozen
                .model_choice
                .validate()
                .map_err(HarnessError::Unsupported)?;
            if frozen.model_choice.provider != AgentProvider::ClaudeAdk {
                return Err(HarnessError::Unsupported(format!(
                    "this is a Claude Code session; a turn choosing {} needs a session on that provider",
                    frozen.model_choice.provider.label()
                )));
            }
            if self.turn_choice_support(&frozen.model_choice) == TurnChoiceSupport::RestartRequired
            {
                return Err(HarnessError::Unsupported(
                    "this claude session takes a new model in place but not a new effort; start a fresh session with the requested choice"
                        .to_string(),
                ));
            }
            self.apply_model(frozen.model_choice.model.as_deref())?;
        }
        let line = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": turn.text }],
            },
        })
        .to_string();
        self.write_turn_line(&line)
    }

    /// Announced by the child in its own `init` line, so the same provider
    /// answers differently on two versions of the same CLI — and only while
    /// there is a turn to stop.
    ///
    /// The interrupt ends a TURN, and background work is not one: a session
    /// whose turn closed over a live task is `Working` and cannot be
    /// interrupted, which is a legal pair and always was — the PTY has reported
    /// it since the field landed, and the composer's gate is `working &&
    /// can_interrupt`, so what it offers there is the plain Send. Both halves
    /// are read under one lock, so this can never disagree with the guard
    /// [`interrupt`](AdkSession::interrupt) reads.
    fn can_interrupt(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.turn_open && state.announces_interrupt()
    }

    /// This carrier has a per-turn settings channel: `set_model`, on the
    /// same pipe the turns go down.
    fn accepts_turn_choice(&self) -> bool {
        true
    }

    fn turn_choice_support(&self, choice: &ModelChoice) -> TurnChoiceSupport {
        self.state.lock().unwrap().turn_choice_support(choice)
    }

    /// One `control_request` line on the same pipe the turns go down, and back.
    ///
    /// No wait for the ack — the contract is `send_turn`'s, for `send_turn`'s
    /// reason. What the ack decides is read later, by the reader thread, when
    /// the result that closes the stopped turn arrives.
    ///
    /// A second ask while one is outstanding replaces it: asking twice to stop
    /// the same turn is one ask.
    ///
    /// A press with no turn open is a press that arrived too late — the control
    /// is offered off a digest up to 1.6s old, so the result can close the turn
    /// inside that window or race the ask by milliseconds. Nothing is recorded
    /// and nothing is written: the turn the human meant to stop is already
    /// over, so the ask is satisfied, and the message the press rode in on is
    /// delivered as the ordinary turn it now is. Recording it would leak the
    /// interrupt into THAT turn, whose own result would then hand `Working` to
    /// nothing and clear its own error; writing it would hand a child that
    /// announced `interrupt_cancel_queued_v1` a request that could take the
    /// queued turn with it.
    fn interrupt(&self) -> Result<(), HarnessError> {
        if !self.state.lock().unwrap().announces_interrupt() {
            return Err(HarnessError::Unsupported(
                "this claude advertises no interrupt — send the message instead: it reaches the running turn at its next step boundary".to_string(),
            ));
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let line = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": { "subtype": "interrupt" },
        })
        .to_string();
        {
            // Recorded before the write rather than after it: the child can
            // answer faster than this thread reaches its next lock, and an ack
            // that arrived before the record existed would read as somebody
            // else's. Under the same lock as the turn it is recorded against,
            // so a result cannot close that turn in between.
            let mut state = self.state.lock().unwrap();
            if !state.turn_open {
                return Ok(());
            }
            state.pending_interrupt = Some(PendingInterrupt {
                request_id,
                acked: false,
                steered: false,
            });
        }
        if let Err(refused) = self.write_line(&line) {
            self.state.lock().unwrap().pending_interrupt = None;
            return Err(refused);
        }
        Ok(())
    }

    /// The name the child gave this conversation in its `init` line — what a
    /// respawn resumes BY NAME, sharper than the cwd heuristic the transcript
    /// probe falls back to.
    fn session_id(&self) -> Option<String> {
        self.state.lock().unwrap().session_id.clone()
    }

    fn surfaces(&self) -> Option<AgentSurfaces> {
        self.state.lock().unwrap().surfaces.snapshot()
    }

    fn surfaces_changed(&self) -> Option<watch::Receiver<u64>> {
        Some(self.revision.subscribe())
    }

    fn active_model(&self) -> Option<String> {
        self.state.lock().unwrap().model.clone()
    }

    /// Reported, never guessed — the difference this session protocol exists for. A model
    /// that reasons for forty minutes without emitting a token is `Working` the
    /// whole time, because the turn it was given has not been answered.
    fn status(&self) -> AgentStatus {
        match self.exit_code() {
            Some(code) => AgentStatus::Ended { code: Some(code) },
            None => self.state.lock().unwrap().live_status(),
        }
    }

    fn status_changed(&self) -> Option<watch::Receiver<SessionStatusSnapshot>> {
        Some(self.status_updates.subscribe())
    }

    /// The age of the last protocol line. The same instrument the PTY answers
    /// with its paint clock, reading the evidence a session protocol actually has.
    fn quiet_for(&self) -> Duration {
        Instant::now().saturating_duration_since(self.state.lock().unwrap().last_line)
    }

    /// Wait out the reap lag: a dying child closes its pipes before the OS makes
    /// its exit status reapable, so one poll can report a harness that is
    /// already gone as still running.
    fn exited_within(&self, timeout: Duration) -> bool {
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

    /// Close stdin, then kill and reap.
    ///
    /// Stdin first because it is how this child is asked to leave — the child
    /// runs turn after turn for exactly as long as its stdin is open. The kill
    /// and the reap follow regardless: killing without collecting the status
    /// leaks one zombie per session on a daemon that never restarts.
    fn end(&self) {
        self.state.lock().unwrap().closed = true;
        self.stdin.lock().unwrap().take();
        let reaped = {
            let mut child = self.child.lock().unwrap();
            let _ = child.kill();
            child.wait().ok().map(observed_code)
        };
        if let Some(code) = reaped {
            let mut cached = self.exit_code.lock().unwrap();
            cached.get_or_insert(code);
            drop(cached);
            publish_status(
                &self.status_updates,
                AgentStatus::Ended { code: Some(code) },
            );
        }
    }

    /// The last error this session was TOLD — a result line that carried one,
    /// or failing that the last thing the child said on stderr.
    ///
    /// Reported, never scraped: there is no screen here, and the sweep that
    /// explains a crash asks the tab's screen first and this second. A
    /// successful result clears the reported error, because an epitaph explains
    /// how the session ENDED and a turn that recovered is not how it ended.
    fn epitaph(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .reported_error
            .clone()
            .or_else(|| state.last_stderr_line.clone())
    }

    fn start_refused(&self) -> Option<String> {
        self.state.lock().unwrap().start_refused.clone()
    }

    /// Everything this session did, on its way to the conversation. It reports
    /// its own reasoning and tool calls, so this is the stream that stands in
    /// for the terminal it does not have.
    fn activity(&self) -> Option<broadcast::Receiver<ActivityReport>> {
        Some(match self.activity.lock().unwrap().as_ref() {
            Some(sender) => sender.subscribe(),
            None => {
                // The child's stdout already ended: hand back an already-closed
                // stream rather than one that will never speak or close.
                let (sender, receiver) = broadcast::channel(1);
                drop(sender);
                receiver
            }
        })
    }

    /// Test-only: age the quiet clock — see the PTY's counterpart. The windows
    /// it feeds are minutes long, and a suite that waited them out in real time
    /// would be unrunnable.
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration) {
        let mut state = self.state.lock().unwrap();
        state.last_line = state
            .last_line
            .checked_sub(ago)
            .expect("a stamp old enough to age");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod commit_tests {
    use super::*;
    use crate::harness::adk::fake::{INIT, RESULT, TASK_NOTIFICATION, TASK_STARTED};
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::sync::mpsc;

    struct ReapChild(Arc<Mutex<Child>>);

    impl Drop for ReapChild {
        fn drop(&mut self) {
            let mut child = self.0.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn release_fifo(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
            {
                Ok(mut gate) => {
                    gate.write_all(b"go\n").unwrap();
                    return;
                }
                Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                    assert!(Instant::now() < deadline, "child never opened {path:?}");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("cannot open {path:?}: {error}"),
            }
        }
    }

    #[test]
    fn full_stdin_pipe_does_not_hold_the_reader_out_of_protocol_state() {
        let dir = tempfile::tempdir().unwrap();
        let first_gate = dir.path().join("report-task");
        let second_gate = dir.path().join("read-turn");
        for path in [&first_gate, &second_gate] {
            let path = CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        let script = format!(
            "printf '%s\\n' '{INIT}'; read gate < '{}'; printf '%s\\n' '{TASK_STARTED}'; read gate < '{}'; printf '%s\\n' '{TASK_NOTIFICATION}'; IFS= read -r turn; printf '%s\\n' '{RESULT}'; while IFS= read -r rest; do :; done",
            first_gate.display(),
            second_gate.display(),
        );
        let spec = HarnessSpec::new("sh").arg("-c").arg(script);
        let choice = ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            ..ModelChoice::default()
        };
        let session = Arc::new(AdkSession::spawn(&spec, None, &choice).unwrap().0);
        let _reap = ReapChild(Arc::clone(&session.child));
        wait_for("init", || session.status() == AgentStatus::Waiting);

        let watched = session.status_updates.subscribe();
        let (capacity, original_flags, reader_fd) = {
            let stdin = session.stdin.lock().unwrap();
            let fd = stdin.as_ref().unwrap().as_raw_fd();
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert!(unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, 4096) } > 0);
            let capacity = unsafe { libc::fcntl(fd, libc::F_GETPIPE_SZ) };
            assert!(capacity > 0);
            let duplicate = unsafe { libc::dup(fd) };
            assert!(duplicate >= 0);
            (capacity as usize, flags, unsafe {
                OwnedFd::from_raw_fd(duplicate)
            })
        };
        let overhead =
            json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":""}]}})
                .to_string()
                .len();
        assert!(capacity > overhead);
        let turn = Turn::new("x".repeat(capacity - overhead));
        let (blocked, delimiter_blocked) = mpsc::channel();
        *session.delimiter_blocked.lock().unwrap() = Some(blocked);
        let sender = Arc::clone(&session);
        let (sent, finished) = mpsc::channel();
        let sending = std::thread::spawn(move || {
            sent.send(sender.send_turn(&turn).map_err(|error| error.to_string()))
                .unwrap();
        });

        wait_for("the user line to fill stdin before its newline", || {
            let mut unread = 0;
            assert_eq!(
                unsafe { libc::ioctl(reader_fd.as_raw_fd(), libc::FIONREAD, &mut unread) },
                0
            );
            unread == capacity as libc::c_int
        });
        delimiter_blocked
            .recv_timeout(Duration::from_secs(5))
            .expect("the final newline finds the pipe full");
        assert!(matches!(
            finished.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_fifo(&first_gate);
        wait_for("the reader to report the task while stdin is full", || {
            watched.borrow().status == AgentStatus::Working
        });
        assert!(matches!(
            finished.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_fifo(&second_gate);
        finished
            .recv_timeout(Duration::from_secs(5))
            .expect("the child drains stdin")
            .expect("the turn is written");
        sending.join().unwrap();
        assert_eq!(
            unsafe { libc::fcntl(reader_fd.as_raw_fd(), libc::F_GETFL) },
            original_flags,
            "the delimiter write restores the pipe's original flags"
        );
        wait_for("the turn result", || {
            watched.borrow().status == AgentStatus::Waiting
        });
        session.end();
    }
}
