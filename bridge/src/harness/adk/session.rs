use super::activity::ActivitySlot;
use super::protocol::{publish_status, PendingInterrupt, ProtocolState, ACTIVITY_BACKLOG};
use super::reader::ProtocolReader;
use crate::harness::surfaces::{AgentSurfaces, SurfaceRevision};
use crate::harness::{
    ActivityReport, AgentSession, AgentStatus, HarnessError, SessionStatusSnapshot, Turn,
};
use crate::pty::HarnessSpec;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};

pub struct AdkSession {
    child: Mutex<Child>,
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
    ) -> Result<(AdkSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        let mut command = Command::new(crate::pty::resolve_binary(spec)?);
        command.args(&spec.args);
        for key in &spec.unset {
            command.env_remove(key);
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let state = Arc::new(Mutex::new(ProtocolState::new()));
        let (sender, subscribed) = broadcast::channel(ACTIVITY_BACKLOG);
        let activity: ActivitySlot = Arc::new(Mutex::new(Some(sender)));
        let revision = SurfaceRevision::default();
        let (status_updates, _) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));

        if let Some(stdout) = child.stdout.take() {
            let mut reader = ProtocolReader::new(
                Arc::clone(&state),
                Arc::clone(&activity),
                revision.clone(),
                status_updates.clone(),
            );
            let slot = Arc::clone(&activity);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) => reader.read_line(&line),
                        Err(_) => break,
                    }
                }
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
        if let Some(stderr) = child.stderr.take() {
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

        let stdin = child.stdin.take();
        Ok((
            AdkSession {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
                state,
                status_updates,
                activity,
                revision,
                exit_code: Mutex::new(None),
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
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        let line = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "text", "text": turn.text }],
            },
        })
        .to_string();
        self.write_line(&line)?;
        let mut state = self.state.lock().unwrap();
        state.turn_open = true;
        // A turn handed over behind an outstanding interrupt is the steering
        // turn: the child runs it once the interrupted one is closed, so the
        // result that closes that one must hand `Working` on to this rather
        // than report a session that is actively working as waiting.
        if let Some(pending) = state.pending_interrupt.as_mut() {
            pending.steered = true;
        }
        publish_status(&self.status_updates, state.live_status());
        Ok(())
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
