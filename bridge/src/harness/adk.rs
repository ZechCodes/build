//! The headless Claude Code carrier — a session protocol, not a terminal.
//!
//! `claude -p --input-format stream-json --output-format stream-json` runs the
//! whole harness over newline-delimited JSON on stdin and stdout: a turn is one
//! `user` line written in, and the child answers with `system` lifecycle lines
//! (`init` carries the session id), `assistant` / `user` messages whose content
//! blocks are text, thinking, tool calls and tool results, and one `result`
//! line per turn — the turn boundary. The child stays alive turn after turn for
//! as long as its stdin is open.
//!
//! So this session answers from what it was TOLD rather than from what it can
//! see: [`status`](AgentSession::status) comes from real turn boundaries
//! instead of a paint clock, [`quiet_for`](AgentSession::quiet_for) from the age
//! of the last protocol line, and [`epitaph`](AgentSession::epitaph) from the
//! last error the child reported. It is not opaque, so it has no basement —
//! [`terminal`](AgentSession::terminal) stays `None` and its reasoning, tool
//! calls and narration reach the conversation through
//! [`activity`](AgentSession::activity).
//!
//! Nothing here parses a screen. Every answer below is a value the child said
//! out loud, which is what keeps the scope doc's no-scraping rule intact for a
//! carrier that has no screen to scrape.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::harness::claude::ClaudeHarness;
use crate::harness::{
    AgentActivity, AgentSession, AgentStatus, Harness, HarnessContext, HarnessError,
    SessionLocator, ToolOutcome, Turn, INHERITED_AGENT_MARKERS,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

/// Claude Code, run headless over its session protocol.
///
/// The same binary, the same account, the same model catalog and the same
/// transcripts on disk as [`ClaudeHarness`] — which is why every one of those
/// answers is delegated to it rather than copied. What this provider decides is
/// the argv, and the one answer that follows from it: `-p` with stream-json on
/// both ends is not a terminal, so there is no basement to offer.
pub struct AdkHarness;

impl Harness for AdkHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::ClaudeAdk
    }

    /// The default carrier of this CLI, so it owns the plain name the human
    /// knows. The terminal carrier beside it is "Claude Code TUI" — the
    /// difference a human can see, never the word the code uses for it.
    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn models(&self) -> Vec<ModelOption> {
        ClaudeHarness.models()
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        ClaudeHarness.effort_levels()
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        ClaudeHarness.model_args(choice)
    }

    /// The interactive argv with the TUI swapped for the protocol, and nothing
    /// else moved.
    ///
    /// The MCP half is identical on purpose — the same per-agent
    /// `--mcp-config`, the same `--strict-mcp-config`, the same
    /// `BRIDGE_MCP_SOCKET` / `BRIDGE_MCP_TOKEN` — because `done`,
    /// `post_thread_message`, `read_unread_messages` and `search_conversation`
    /// arrive over the same unix socket whichever carrier is running. No
    /// settle window and no submit delay: those are how a prompt is typed into
    /// a line editor, and this harness is handed a turn as a value.
    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> HarnessSpec {
        let mut spec = HarnessSpec::new("claude")
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            // Without it the child emits only the final result of each turn,
            // and the conversation would learn what the agent did after it had
            // finished doing it.
            .arg("--verbose")
            .arg("--mcp-config")
            .arg(crate::orchestrator::mcp_config_path(&options.owner_id))
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions");
        // The two are alternatives and never both: `--resume` names the exact
        // conversation this agent was having, `--continue` guesses the newest
        // one in the checkout, and passing both would ask for two different
        // conversations. The name wins where there is one; the guess is what
        // answers for a session that died before it could say its own.
        match options.resume_session_id.as_deref() {
            Some(named) => spec = spec.arg("--resume").arg(named),
            None if options.continue_session => spec = spec.arg("--continue"),
            None => {}
        }
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        spec.env("BRIDGE_MCP_SOCKET", &context.mcp_socket)
            .env("BRIDGE_MCP_TOKEN", &options.mcp_session_token)
    }

    /// No terminal, and this is the first provider to say so. A session that
    /// reports its own reasoning and tool calls is not opaque, so there is
    /// nothing for a human to escape to — the rail offers no TUI button and the
    /// terminal verbs refuse.
    fn has_terminal(&self) -> bool {
        false
    }

    /// The trust registry is the CLI's, not the TUI's: a headless session in an
    /// untrusted directory is refused the same way, and the refusal is worse
    /// here because there is no screen to show it on.
    fn prepare_workspace(&self, cwd: &Path) {
        ClaudeHarness.prepare_workspace(cwd)
    }

    /// Headless sessions write the same `~/.claude/projects/**/*.jsonl`
    /// transcripts the TUI does, so the resume question has the same answer —
    /// and a worktree the human left a conversation in is picked up whichever
    /// carrier ran there.
    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        ClaudeHarness.has_transcript(home, cwd)
    }

    /// The same transcripts, so the same answer — and this is the dividend: an
    /// id captured under the TUI carrier verifies here and resumes here, and
    /// one captured here resumes there. Both write that tree and both spend
    /// `--resume`, so a provider swap between the two claude carriers keeps the
    /// exact conversation.
    fn holds_conversation(&self, home: &Path, cwd: &Path, id: &str) -> bool {
        ClaudeHarness.holds_conversation(home, cwd, id)
    }

    /// No locator, and it is the only provider that answers so. This session
    /// reads its own id off the `init` line the child sent, and a locator
    /// beside that would be two records of one answer, free to disagree.
    fn session_locator(&self, home: &Path, cwd: &Path) -> Option<Box<dyn SessionLocator>> {
        let _ = (home, cwd);
        None
    }
}

/// How Build's own MCP tools are named once the harness has loaded them.
///
/// The orchestrator scaffolds one server per agent under the name `build`
/// (`scaffold_agent_worktree`), and claude exposes a server's tools as
/// `mcp__<server>__<tool>`. Those calls are excluded from activity: `done` and
/// `post_thread_message` arrive over the socket as their real selves, so minting
/// the call as well would tell the timeline everything twice.
const BUILD_MCP_TOOL_PREFIX: &str = "mcp__build__";

/// How much of a tool call's input, or of a tool's answer, one summary carries.
///
/// A tool call is machinery, not something the agent said: a `Write` input is an
/// entire file and a `Read` result can be thousands of lines, and neither
/// belongs in a conversation row whole. Reasoning and narration are NOT capped
/// here — those are the agent's own words, and the conversation carries what an
/// agent says whole.
const TOOL_SUMMARY_LIMIT: usize = 240;

/// How large the activity backlog may grow before a slow subscriber loses the
/// oldest events. Matches the byte pump's window: a turn that calls forty tools
/// while nothing is draining is a reader problem, not a reason to stall the
/// child.
const ACTIVITY_BACKLOG: usize = 1024;

/// The capability a child announces in its `init` line when the turn it is
/// running can be stopped over the wire.
///
/// Asked of the child rather than of a version number, because the same CLI
/// answers differently on two releases — and a Build that guessed from the
/// version would offer a control the session then refused.
const INTERRUPT_CAPABILITY: &str = "interrupt_receipt_v1";

/// The interrupt Build asked for, until the result that closes the turn it
/// ended arrives.
struct PendingInterrupt {
    request_id: String,
    /// The child answered `control_response` for this id — which it does before
    /// it emits the result, so an interrupt still unacked when the result lands
    /// is one the child never acted on.
    acked: bool,
    /// A turn was handed over behind the interrupt — the steering turn, which
    /// the child runs once the interrupted one is closed.
    steered: bool,
}

/// Everything the protocol has told this session so far.
///
/// Every field is reported rather than inferred, which is the whole difference
/// between this carrier and a terminal: a PTY guesses `Working` from the age of
/// the last byte, and this one counts turns against the results that closed
/// them.
struct ProtocolState {
    /// Whether the child has announced itself (`system` / `init`). Until it
    /// does, the session is starting and cannot take a turn.
    announced: bool,
    /// Whether a turn Build handed over is still unanswered.
    ///
    /// A flag rather than a count of turns against results, and the protocol is
    /// why: a message written while the child is mid-turn is absorbed into the
    /// running turn, which still ends in ONE result line. Counting would leave
    /// such a session reporting `Working` for the rest of its life — and since
    /// `Working` short-circuits the idle sweep, an agent that quietly stopped
    /// would never be explained.
    turn_open: bool,
    /// When the child last said anything at all — the quiet clock's instant.
    last_line: Instant,
    /// The id the child gave this conversation, for `--resume`.
    session_id: Option<String>,
    /// What the child announced it can do, verbatim from its `init` line.
    capabilities: Vec<String>,
    /// The interrupt Build is waiting on, if any. At most one: asking twice to
    /// stop the same turn is one ask.
    pending_interrupt: Option<PendingInterrupt>,
    /// The last error the child REPORTED, from a result line that carried one.
    reported_error: Option<String>,
    /// The last thing the child said on stderr, for a death with no result.
    last_stderr_line: Option<String>,
    /// The background work the child says is live right now — its task id
    /// against the description it goes by.
    ///
    /// Reconciled against the child's own roster rather than bookkept from the
    /// start and end events, so it cannot drift from what the harness says is
    /// running. Sorted rather than hashed because the rows it mints are read by
    /// a human in the order they are minted, and a hash order would shuffle two
    /// tasks ending together from run to run.
    tasks: BTreeMap<String, String>,
}

impl ProtocolState {
    fn new() -> ProtocolState {
        ProtocolState {
            announced: false,
            turn_open: false,
            last_line: Instant::now(),
            session_id: None,
            capabilities: Vec::new(),
            pending_interrupt: None,
            reported_error: None,
            last_stderr_line: None,
            tasks: BTreeMap::new(),
        }
    }

    /// What a session that has not exited is doing, straight from its own turn
    /// boundaries: starting until it announces itself, working while a turn it
    /// accepted is unanswered OR background work it started is still running,
    /// waiting for the human otherwise.
    ///
    /// The task half is what ends the headless-looks-idle failure: a turn can
    /// close over work that outlives it, and an agent reported `Waiting` there
    /// went dark on the rail while it was still doing something. Nothing else
    /// clears the set — reconciliation, a terminal task event and the session
    /// ending are its only exits — so a session cannot be pinned `Working` by
    /// work that is over.
    fn live_status(&self) -> AgentStatus {
        if self.turn_open || !self.tasks.is_empty() {
            AgentStatus::Working
        } else if self.announced {
            AgentStatus::Waiting
        } else {
            AgentStatus::Starting
        }
    }

    /// Whether the child announced it can stop a turn. Asked apart from
    /// [`AgentSession::can_interrupt`] because the two questions differ: this
    /// one decides whether Build may write a `control_request` at all, and the
    /// public one also asks whether there is a turn to spend it on.
    fn announces_interrupt(&self) -> bool {
        self.capabilities
            .iter()
            .any(|announced| announced == INTERRUPT_CAPABILITY)
    }
}

/// What became of one tool call, kept until its result arrives so the answer
/// can be paired to it — or so it can be closed as unanswered when the turn
/// ends first. A call that was Build's own is remembered too, so that its answer
/// stays as silent as the call was.
#[derive(PartialEq, Eq)]
enum RecordedCall {
    Minted,
    BuildsOwn,
}

/// The broadcast side of the activity stream, dropped when the child's stdout
/// ends so every subscriber observes the close.
///
/// The close is load-bearing: a no-terminal session's death rites hang off it
/// exactly the way the byte pump's hang off PTY EOF, so a stream that never
/// closed would leave a dead agent's tab reading as live.
type ActivitySlot = Arc<Mutex<Option<broadcast::Sender<AgentActivity>>>>;

/// A live headless session: a child with piped stdio, one reader per stream, and
/// everything the protocol has said so far.
pub struct AdkSession {
    child: Mutex<Child>,
    /// `None` once the session has ended — the pipe is dropped so the child
    /// sees EOF and can leave on its own terms before it is killed.
    stdin: Mutex<Option<ChildStdin>>,
    state: Arc<Mutex<ProtocolState>>,
    activity: ActivitySlot,
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
    ) -> Result<(AdkSession, broadcast::Receiver<AgentActivity>), HarnessError> {
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

        if let Some(stdout) = child.stdout.take() {
            let mut reader = ProtocolReader {
                state: Arc::clone(&state),
                activity: Arc::clone(&activity),
                calls: HashMap::new(),
            };
            let slot = Arc::clone(&activity);
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(line) => reader.read_line(&line),
                        Err(_) => break,
                    }
                }
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
                activity,
                exit_code: Mutex::new(None),
            },
            subscribed,
        ))
    }

    /// Write one protocol line to the child's stdin, and return.
    ///
    /// The whole of what this carrier says to its child — a turn, an interrupt
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
        *cached
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

    /// Reported, never guessed — the difference this carrier exists for. A model
    /// that reasons for forty minutes without emitting a token is `Working` the
    /// whole time, because the turn it was given has not been answered.
    fn status(&self) -> AgentStatus {
        match self.exit_code() {
            Some(code) => AgentStatus::Ended { code: Some(code) },
            None => self.state.lock().unwrap().live_status(),
        }
    }

    /// The age of the last protocol line. The same instrument the PTY answers
    /// with its paint clock, reading the evidence this carrier actually has.
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
    /// Stdin first because it is how this carrier is asked to leave — the child
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
    fn activity(&self) -> Option<broadcast::Receiver<AgentActivity>> {
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

/// Reads the child's stream-json output, updates what the session knows and
/// mints the conversation's activity events.
///
/// It owns the tool-call map because that is the only place the pairing lives: a
/// `tool_result` names the call it answers by id and nothing else, so the tool's
/// name — and whether the call was Build's own and therefore never minted — has
/// to be remembered from the call until its answer arrives.
struct ProtocolReader {
    state: Arc<Mutex<ProtocolState>>,
    activity: ActivitySlot,
    calls: HashMap<String, RecordedCall>,
}

impl ProtocolReader {
    fn read_line(&mut self, line: &str) {
        self.state.lock().unwrap().last_line = Instant::now();
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            // Not protocol. A harness can print a warning to stdout before the
            // stream starts; it is evidence the child is alive (stamped above)
            // and nothing more.
            return;
        };
        match event["type"].as_str() {
            Some("system") => self.read_system(&event),
            Some("assistant") => self.read_message(&event, Voice::Assistant),
            Some("user") => self.read_message(&event, Voice::User),
            Some("result") => self.read_result(&event),
            Some("control_response") => self.read_control_response(&event),
            _ => {}
        }
    }

    /// The lifecycle line, and the background-task lines that ride the same
    /// subtype. Anything else on `system` is not this session's business.
    fn read_system(&mut self, event: &Value) {
        match event["subtype"].as_str() {
            Some("init") => self.read_init(event),
            Some("background_tasks_changed") => self.read_task_roster(event),
            Some("task_started") => self.read_task_started(event),
            Some("task_updated") => self.read_task_updated(event),
            Some("task_notification") => self.read_task_notification(event),
            _ => {}
        }
    }

    /// `init` is when the child can take a turn, and it carries the session id a
    /// respawn resumes by.
    fn read_init(&mut self, event: &Value) {
        let mut state = self.state.lock().unwrap();
        state.announced = true;
        if let Some(id) = event["session_id"].as_str() {
            state.session_id = Some(id.to_string());
        }
        if let Some(announced) = event["capabilities"].as_array() {
            state.capabilities = announced
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect();
        }
    }

    /// The child's own statement of what background work is live, which
    /// REPLACES the set rather than merging into it.
    ///
    /// A reconciled set cannot drift from the harness: a task Build somehow
    /// never saw start is inserted here, and a task whose end never got its own
    /// event is removed here. Both are membership transitions, so both mint.
    fn read_task_roster(&mut self, event: &Value) {
        let listed: Vec<(String, String)> = event["tasks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|task| {
                let id = task["task_id"].as_str()?;
                Some((id.to_string(), task_description(task, id)))
            })
            .collect();
        let minted = {
            let mut state = self.state.lock().unwrap();
            let mut minted = Vec::new();
            for (id, description) in &listed {
                if !state.tasks.contains_key(id) {
                    minted.push(format!("{description} — started"));
                }
            }
            for (id, description) in &state.tasks {
                if !listed.iter().any(|(listed, _)| listed == id) {
                    minted.push(format!("{description} — finished"));
                }
            }
            state.tasks = listed.into_iter().collect();
            minted
        };
        self.mint_task_updates(minted);
    }

    /// A task announcing itself. The minting trigger for a start, and the reason
    /// status flips to `Working` without waiting for the next roster — but only
    /// when it actually inserts, because a roster that already listed this task
    /// has said the same thing once.
    fn read_task_started(&mut self, event: &Value) {
        let Some(id) = event["task_id"].as_str() else {
            return;
        };
        let description = task_description(event, id);
        let minted = {
            let mut state = self.state.lock().unwrap();
            match state.tasks.insert(id.to_string(), description.clone()) {
                Some(_) => Vec::new(),
                None => vec![format!("{description} — started")],
            }
        };
        self.mint_task_updates(minted);
    }

    /// A patch against one task. A terminal status ends it; anything else is
    /// progress and touches membership not at all.
    ///
    /// A progress patch mints nothing. The pinned payload carries only a status
    /// and an end time — no human-readable line of its own — so a patch that
    /// moves no membership has nothing to say that the task's own name did not
    /// already say. A `description` it does carry renames the task for the rows
    /// still to come rather than minting a row about the rename.
    fn read_task_updated(&mut self, event: &Value) {
        let Some(id) = event["task_id"].as_str() else {
            return;
        };
        let patch = &event["patch"];
        let status = patch["status"]
            .as_str()
            .or_else(|| event["status"].as_str())
            .unwrap_or_default();
        let minted = {
            let mut state = self.state.lock().unwrap();
            if !task_status_is_terminal(status) {
                if let Some(renamed) = patch["description"].as_str() {
                    if let Some(held) = state.tasks.get_mut(id) {
                        *held = renamed.to_string();
                    }
                }
                Vec::new()
            } else {
                match state.tasks.remove(id) {
                    Some(description) => vec![ended_summary(status, &description, patch)],
                    None => Vec::new(),
                }
            }
        };
        self.mint_task_updates(minted);
    }

    /// The task saying something worth reading — and, when it carries a
    /// terminal status, the only word some tasks ever get that the work is over.
    ///
    /// A FOREGROUND Bash command is a task too, and the child closes it with a
    /// notification alone: no `task_updated`, no roster, ever (probe,
    /// 2026-08-30). A reader that took every notification for chatter would
    /// hold that task for the life of the session and report `Working` over an
    /// agent idle for hours — the inverse of the failure this step closes. So a
    /// terminal status here IS a membership removal, and mints the ending row
    /// the way the roster and the terminal patch do.
    ///
    /// Its text is minted under the task's own name while the set still holds
    /// it, and on its own after that: the child empties the roster before it
    /// delivers a background task's notification, and a name the set no longer
    /// holds is not a name to speak with — the text says which task it is
    /// either way. Text that only repeats the task's own name mints nothing,
    /// because a foreground notification's summary IS the description, and a
    /// row reading `X: X` says nothing the ending row did not.
    fn read_task_notification(&mut self, event: &Value) {
        let said = event["summary"]
            .as_str()
            .or_else(|| event["message"].as_str())
            .unwrap_or_default()
            .trim();
        let status = event["status"].as_str().unwrap_or_default();
        let ends = task_status_is_terminal(status);
        let minted = {
            let mut state = self.state.lock().unwrap();
            let held = event["task_id"].as_str().and_then(|id| match ends {
                true => state.tasks.remove(id),
                false => state.tasks.get(id).cloned(),
            });
            let mut minted = Vec::new();
            if !said.is_empty() && held.as_deref() != Some(said) {
                minted.push(match &held {
                    Some(description) => format!("{description}: {said}"),
                    None => said.to_string(),
                });
            }
            if ends {
                if let Some(description) = &held {
                    minted.push(ended_summary(status, description, event));
                }
            }
            minted
        };
        self.mint_task_updates(minted);
    }

    /// Send one row per transition, in the order the transitions happened, each
    /// clipped the way a tool summary is: this is operational text about the
    /// work, not the agent speaking.
    fn mint_task_updates(&self, summaries: Vec<String>) {
        for summary in summaries {
            self.emit(AgentActivity::TaskUpdate {
                summary: one_line(&summary, TOOL_SUMMARY_LIMIT),
            });
        }
    }

    /// The child's answer to a `control_request`. Only the outstanding
    /// interrupt's own id counts: a response naming another request is noise,
    /// and a session that took it as its own would swallow a real crash.
    fn read_control_response(&mut self, event: &Value) {
        let Some(answered) = event["response"]["request_id"]
            .as_str()
            .or_else(|| event["request_id"].as_str())
        else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending_interrupt.as_mut() {
            if pending.request_id == answered {
                pending.acked = true;
            }
        }
    }

    /// The turn boundary. A result is never a completion — `done` is still the
    /// only completion contract — so this closes the turn and, when it carried
    /// an error, records the session's last words.
    ///
    /// Unless the human stopped it. An interrupted turn ends in an
    /// `error_during_execution` result, and reporting that as a crash would end
    /// the human's own stop with a crash notice quoting it. The ACK is what
    /// makes the clearing safe rather than a blanket amnesty: the child answers
    /// the control request before it emits the result, so an interrupt still
    /// unanswered here is one the child never acted on, and the failure the
    /// result reports is the turn's own.
    fn read_result(&mut self, event: &Value) {
        let failed = event["is_error"].as_bool().unwrap_or(false)
            || event["subtype"]
                .as_str()
                .is_some_and(|kind| kind != "success");
        {
            let mut state = self.state.lock().unwrap();
            // Taken, acked or not, so an interrupt can never leak into the turn
            // after the one it ended.
            let stopped = state.pending_interrupt.take();
            // The turn queued behind an interrupt is running the moment this
            // result lands, so the flag is handed to it rather than cleared.
            state.turn_open = stopped.as_ref().is_some_and(|pending| pending.steered);
            state.reported_error =
                match failed && !stopped.as_ref().is_some_and(|pending| pending.acked) {
                    true => Some(result_error_text(event)),
                    false => None,
                };
        }
        // Outside the lock, because emitting is the broadcast channel's
        // business and not this session's state. A turn the protocol answered
        // in full leaves nothing to close.
        self.close_open_calls();
    }

    /// One message's content blocks, minted in the order the child reported
    /// them.
    ///
    /// The voice decides what a block can be: text and thinking are the agent
    /// speaking, so they are only read off an `assistant` message — a `user`
    /// message carrying text is Build's own turn echoed back, and minting that
    /// would put the human's words in the timeline a second time as narration.
    fn read_message(&mut self, event: &Value, voice: Voice) {
        // A subagent's own reasoning is folded into the tool call that spawned
        // it rather than minted beside it: the human reads one tool call, not
        // two conversations interleaved.
        if event["parent_tool_use_id"].as_str().is_some() {
            return;
        }
        let Some(blocks) = event["message"]["content"].as_array() else {
            return;
        };
        for block in blocks {
            match (voice, block["type"].as_str()) {
                (Voice::Assistant, Some("thinking")) => {
                    if let Some(summary) = spoken(block["thinking"].as_str()) {
                        self.emit(AgentActivity::Reasoning { summary });
                    }
                }
                (Voice::Assistant, Some("text")) => {
                    if let Some(summary) = spoken(block["text"].as_str()) {
                        self.emit(AgentActivity::Narration { summary });
                    }
                }
                (Voice::Assistant, Some("tool_use")) => self.read_tool_use(block),
                (Voice::User, Some("tool_result")) => self.read_tool_result(block),
                _ => {}
            }
        }
    }

    fn read_tool_use(&mut self, block: &Value) {
        let tool = block["name"].as_str().unwrap_or_default().to_string();
        let call_id = block["id"].as_str().unwrap_or_default().to_string();
        if tool.starts_with(BUILD_MCP_TOOL_PREFIX) {
            self.calls.insert(call_id, RecordedCall::BuildsOwn);
            return;
        }
        let summary = tool_call_summary(&tool, &block["input"]);
        self.calls.insert(call_id.clone(), RecordedCall::Minted);
        self.emit(AgentActivity::ToolUse { call_id, summary });
    }

    /// One call's answer, reported as the completion of the call it names
    /// rather than as an event of its own — the pairing this reader has always
    /// computed, carried outward instead of thrown away.
    ///
    /// The answer travels alone, without the tool's name in front of it: the row
    /// it lands on is the call, which said what tool this was when it was
    /// minted.
    fn read_tool_result(&mut self, block: &Value) {
        let call_id = block["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Taken, not read: a call is answered once, and a session that runs for
        // hours must not accumulate one entry per tool call it ever made.
        if self.calls.remove(&call_id) == Some(RecordedCall::BuildsOwn) {
            return;
        }
        let outcome = match block["is_error"].as_bool().unwrap_or(false) {
            true => ToolOutcome::Error,
            false => ToolOutcome::Ok,
        };
        self.emit(AgentActivity::ToolResult {
            call_id,
            outcome,
            summary: one_line(&tool_result_text(block), TOOL_SUMMARY_LIMIT),
        });
    }

    /// Close every call the turn just ended left open.
    ///
    /// A call whose answer never came must not go on claiming to run, so the
    /// boundary that ended it says so: one `Unanswered` completion per call
    /// still in the map, and Build's own calls dropped in the silence their
    /// answers always kept. Draining here is also what leaves the next turn
    /// reading against an empty map.
    fn close_open_calls(&mut self) {
        for (call_id, recorded) in std::mem::take(&mut self.calls) {
            if recorded == RecordedCall::BuildsOwn {
                continue;
            }
            self.emit(AgentActivity::ToolResult {
                call_id,
                outcome: ToolOutcome::Unanswered,
                summary: String::new(),
            });
        }
    }

    /// Hand one event to whoever is listening. A send with no subscriber, or a
    /// backlog nobody drained, is not the child's problem: the protocol is read
    /// at the speed the child speaks it either way.
    fn emit(&self, activity: AgentActivity) {
        if let Some(sender) = self.activity.lock().unwrap().as_ref() {
            let _ = sender.send(activity);
        }
    }
}

/// Who a message came from, which decides what its blocks can mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Voice {
    Assistant,
    User,
}

/// What the agent actually said in a content block, or `None` for a block with
/// nothing in it — an empty block is not something the agent said.
fn spoken(text: Option<&str>) -> Option<String> {
    let text = text.unwrap_or_default().trim();
    match text.is_empty() {
        true => None,
        false => Some(text.to_string()),
    }
}

/// The one argument each tool is worth reading by — its meat key, matched on
/// the name the protocol calls the tool.
///
/// One table, read by one function: a row is one quiet line, so a call named
/// here mints this field and drops everything else it carried. `Bash`'s
/// `description` is dropped deliberately — it is the model's paraphrase where
/// the command is the record, and two claims about one act are worse than one.
const TOOL_MEAT_KEYS: &[(&str, &str)] = &[
    ("Bash", "command"),
    ("Read", "file_path"),
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("NotebookEdit", "notebook_path"),
    ("Glob", "pattern"),
    ("Grep", "pattern"),
    ("WebFetch", "url"),
    ("WebSearch", "query"),
    ("Task", "description"),
];

/// A tool call on one line: the tool's name, plus the thing it acted on —
/// `Bash cargo test`, `Read bridge/src/app.rs`.
///
/// The name still leads, because the row's icon says only "a tool call" and
/// `Edit foo.rs` against `Read foo.rs` is a distinction worth five characters.
fn tool_call_summary(tool: &str, input: &Value) -> String {
    let meat = one_line(&tool_call_meat(tool, input), TOOL_SUMMARY_LIMIT);
    match meat.is_empty() {
        true => tool.to_string(),
        false => format!("{tool} {meat}"),
    }
}

/// What a call is worth reading: its tool's meat key when [`TOOL_MEAT_KEYS`]
/// names one and the call carried it as a string, else the first string-valued
/// field the input holds, else nothing at all.
///
/// Never JSON. A tool the table has not heard of — an MCP tool, or one newer
/// than this table — is guessed at rather than rendered as an object, because a
/// truncated-but-human line beats a line of punctuation nobody can scan: the row
/// is a scent, and the fold body and the diff are the record. Fields iterate in
/// key order, so which one a guess lands on is a property of the call rather
/// than of how the child happened to spell it.
fn tool_call_meat(tool: &str, input: &Value) -> String {
    let Some(fields) = input.as_object() else {
        return input.as_str().unwrap_or_default().to_string();
    };
    TOOL_MEAT_KEYS
        .iter()
        .find(|(named, _)| *named == tool)
        .and_then(|(_, key)| fields.get(*key)?.as_str())
        .or_else(|| fields.values().find_map(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// What a tool answered. The protocol allows both shapes — a plain string, or
/// the content blocks a richer tool returns — so both are read.
fn tool_result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|inner| inner["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// What a task goes by in the timeline: the description the child gave it,
/// falling back to its id. A row reading `bi1jfa1kd — started` says less than
/// one naming the work, and far more than ` — started`.
fn task_description(event: &Value, id: &str) -> String {
    let described = event["description"].as_str().unwrap_or_default().trim();
    match described.is_empty() {
        false => described.to_string(),
        true => id.to_string(),
    }
}

/// Whether a reported task status means the work is over.
///
/// Named rather than inferred from the absence of a running status, so an
/// unrecognised status leaves the task in the set instead of closing it — where
/// the roster, when one is coming, will close it on the child's own word.
///
/// `completed`, `failed`, `killed` and `stopped` are the four the live probes
/// turned up; the rest are the shapes their names imply, recognised so a task
/// ending under one of them is not held open waiting for a roster that, for a
/// foreground task, never comes.
pub(crate) fn task_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "failed"
            | "error"
            | "cancelled"
            | "canceled"
            | "killed"
            | "stopped"
            | "timed_out"
    )
}

pub(crate) fn task_status_failed(status: &str) -> bool {
    matches!(status, "failed" | "error" | "timed_out")
}

/// The row a task's ending mints: `failed` when the event that ended it said
/// so, with the error it named, and `finished` otherwise. A task that was
/// cancelled, killed or stopped did not fail — something ended it, which is not
/// the same thing to read.
///
/// `ending` is whichever event carried the terminal status: a `task_updated`'s
/// patch, or a `task_notification` itself, which carries its status at the top
/// level and — as the probes recorded it — no error text at all.
fn ended_summary(status: &str, description: &str, ending: &Value) -> String {
    if !task_status_failed(status) {
        return format!("{description} — finished");
    }
    let reported = ending["error"]
        .as_str()
        .or_else(|| ending["result"].as_str())
        .unwrap_or_default()
        .trim();
    match reported.is_empty() {
        true => format!("{description} — failed"),
        false => format!("{description} — failed: {reported}"),
    }
}

/// The error text a failed result carried, falling back to its subtype: an
/// epitaph naming `error_max_turns` explains more than an empty string does.
fn result_error_text(event: &Value) -> String {
    let reported = event["result"]
        .as_str()
        .or_else(|| event["error"].as_str())
        .unwrap_or_default()
        .trim();
    match reported.is_empty() {
        false => one_line(reported, TOOL_SUMMARY_LIMIT),
        true => event["subtype"]
            .as_str()
            .unwrap_or("the session failed without saying why")
            .to_string(),
    }
}

/// `text` collapsed onto one line and clipped to `limit` characters. Clipped by
/// characters rather than bytes: tool output is arbitrary UTF-8, and a byte
/// truncation would split one.
fn one_line(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut clipped: String = collapsed.chars().take(limit).collect();
    clipped.push('…');
    clipped
}

/// A headless child that says what claude says, without a model behind it.
///
/// It lives beside the reader it exercises rather than inside one test module
/// because two suites need the same child: this module's tests, which read one
/// session's protocol, and the daemon's, which drive a headless agent through
/// the whole spawn path. One recording, so what the daemon is tested against
/// cannot drift from what the session is tested against — and a real model turn
/// is never run in either.
#[cfg(test)]
pub(crate) mod fake {
    use crate::pty::HarnessSpec;

    /// The protocol lines a real headless session emits, recorded so the fake
    /// harness below replays exactly what claude would say. Single quotes are
    /// forbidden inside them: the fake is a `sh -c` script that quotes each
    /// line, and a stray quote would rewrite the protocol rather than fail.
    pub(crate) const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-adk","model":"claude-fable-5","capabilities":["msg_lifecycle_v1","interrupt_receipt_v1","interrupt_cancel_queued_v1"]}"#;
    /// The same child on a CLI built before the interrupt landed: it announces
    /// itself and names no capabilities at all. What a refusal is tested
    /// against, and the reason the question is asked of the child rather than
    /// of a version number.
    pub(crate) const INIT_WITHOUT_INTERRUPT: &str = r#"{"type":"system","subtype":"init","session_id":"sess-adk","model":"claude-fable-5","capabilities":["msg_lifecycle_v1"]}"#;
    pub(crate) const THINKING: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"the index is unused"}]},"parent_tool_use_id":null}"#;
    pub(crate) const TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"bridge/src/app.rs"}}]},"parent_tool_use_id":null}"#;
    pub(crate) const TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"fn main() {}"}]},"parent_tool_use_id":null}"#;
    /// A call carrying more than one argument — the recorded shape above with
    /// its id, tool name and input swapped for the two fields the `Bash` tool
    /// takes, as the live CLI sends them. What a row minted from a multi-field
    /// call is read against: the command, and not the model's paraphrase of it.
    pub(crate) const BASH_TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_bash","name":"Bash","input":{"command":"cargo test","description":"Run the test suite"}}]},"parent_tool_use_id":null}"#;
    /// An answer that FAILED — recorded from a live probe against claude
    /// 2.1.236 on 2026-08-30: one headless turn that read a path which does not
    /// exist. `is_error` is the whole of what says so, and the block carries no
    /// other marker.
    ///
    /// The same substitutions the recordings above take and nothing else: the
    /// `tool_use_id` is swapped for this module's, and the probe's own working
    /// directory for a short path. Every field the reader looks at is verbatim.
    pub(crate) const ERROR_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"File does not exist. Note: your current working directory is /work.","is_error":true,"tool_use_id":"toolu_1"}]},"parent_tool_use_id":null}"#;
    /// A SECOND call and its answer, the recorded pair above with its id and
    /// the file it reads changed and nothing else — what an out-of-order
    /// interleave is told apart by, since adjacency cannot tell it.
    pub(crate) const SECOND_TOOL_USE: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Read","input":{"file_path":"bridge/src/thread.rs"}}]},"parent_tool_use_id":null}"#;
    pub(crate) const SECOND_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"pub struct Thread"}]},"parent_tool_use_id":null}"#;
    /// An answer to a call this session never saw announced — the shape a
    /// broadcast lag or a reload leaves behind, and the only thing that still
    /// mints a `tool_result` row of its own.
    pub(crate) const ORPHAN_TOOL_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_nobody_announced","content":"an answer to nothing"}]},"parent_tool_use_id":null}"#;
    pub(crate) const NARRATION: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"dropped the index"}]},"parent_tool_use_id":null}"#;
    pub(crate) const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"dropped the index","session_id":"sess-adk"}"#;
    pub(crate) const DONE_CALL: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_done","name":"mcp__build__done","input":{"phase":"build","status":"completed"}}]},"parent_tool_use_id":null}"#;
    pub(crate) const DONE_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_done","content":"recorded"}]},"parent_tool_use_id":null}"#;
    pub(crate) const SUBAGENT_TEXT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a subagent talking"}]},"parent_tool_use_id":"toolu_1"}"#;
    pub(crate) const FAILED_RESULT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"the tool call was refused","session_id":"sess-adk"}"#;

    /// The background-task lines, recorded from a live probe against claude
    /// 2.1.236 on 2026-08-29: one headless turn that put `sleep 12 && echo
    /// woke` in the background and wrote two lines of haiku.
    ///
    /// The child emitted them in exactly this order — roster, `task_started`,
    /// the turn's `result`, empty roster, `task_updated`, `task_notification`
    /// — which is what [`the_probes_own_order_mints_one_row_per_transition`]
    /// replays. Two substitutions and nothing else: the recorded `session_id`
    /// and `uuid` values are swapped for this module's, and the notification's
    /// `output_file` for a short path, so the recordings read as one session
    /// and carry no machine paths. Every field the reader looks at is verbatim.
    pub(crate) const TASK_ROSTER: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"bi1jfa1kd","task_type":"local_bash","description":"Sleep 12 seconds then echo woke"}],"uuid":"task-uuid-1","session_id":"sess-adk"}"#;
    pub(crate) const TASK_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","description":"Sleep 12 seconds then echo woke","task_type":"local_bash","uuid":"task-uuid-2","session_id":"sess-adk"}"#;
    /// The roster with nothing on it — how the live child says the work is
    /// over, ahead of the `task_updated` that says which way it went.
    pub(crate) const TASK_ROSTER_EMPTY: &str = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[],"uuid":"task-uuid-3","session_id":"sess-adk"}"#;
    pub(crate) const TASK_UPDATED_DONE: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"status":"completed","end_time":1788057950364},"uuid":"task-uuid-4","session_id":"sess-adk"}"#;
    pub(crate) const TASK_NOTIFICATION: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","status":"completed","output_file":"/tmp/tasks/bi1jfa1kd.output","summary":"Background command \"Sleep 12 seconds then echo woke\" completed (exit code 0)","uuid":"task-uuid-5","session_id":"sess-adk"}"#;

    /// Three the probe's own command had no reason to produce, each the
    /// recorded shape above with one field changed and nothing else: a task
    /// that FAILED rather than completed, a patch that reports progress rather
    /// than an ending, and a notification whose text runs over several lines
    /// the way a command quoting its own output would.
    pub(crate) const TASK_UPDATED_FAILED: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"status":"failed","error":"exit code 1","end_time":1788057950364},"uuid":"task-uuid-6","session_id":"sess-adk"}"#;
    pub(crate) const TASK_UPDATED_PROGRESS: &str = r#"{"type":"system","subtype":"task_updated","task_id":"bi1jfa1kd","patch":{"output_lines":12},"uuid":"task-uuid-7","session_id":"sess-adk"}"#;
    pub(crate) const TASK_NOTIFICATION_MULTILINE: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bi1jfa1kd","tool_use_id":"toolu_bg","status":"completed","output_file":"/tmp/tasks/bi1jfa1kd.output","summary":"Background command completed\n\n  woke\n","uuid":"task-uuid-8","session_id":"sess-adk"}"#;

    /// What the recorded task calls itself — the human-readable half of every
    /// summary the reader mints for it.
    pub(crate) const TASK_DESCRIPTION: &str = "Sleep 12 seconds then echo woke";

    /// A FOREGROUND Bash command is a task too, and it ends differently —
    /// recorded from a second live probe against claude 2.1.236 on 2026-08-30:
    /// one headless turn that ran a plain `sleep 10` and answered after it.
    ///
    /// The child emitted exactly two task lines for it, in this order:
    /// `task_started`, then a `task_notification` carrying `status`
    /// `completed`. No `task_updated` and no `background_tasks_changed`, ever —
    /// so the notification is the ONLY event that says the work is over, and a
    /// reader that took it for chatter would hold the task for the life of the
    /// session. Note the `summary`: for a foreground task it is the
    /// description, word for word, which is why text that only repeats the
    /// task's own name mints no row of its own.
    ///
    /// The same substitutions as the recordings above and nothing else — the
    /// `session_id`, `uuid` and `tool_use_id` values are swapped for this
    /// module's. Every field the reader looks at is verbatim.
    pub(crate) const FOREGROUND_TASK_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","description":"Sleep for 10 seconds","task_type":"local_bash","uuid":"task-uuid-9","session_id":"sess-adk"}"#;
    pub(crate) const FOREGROUND_TASK_NOTIFICATION: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","status":"completed","output_file":"","summary":"Sleep for 10 seconds","uuid":"task-uuid-10","session_id":"sess-adk"}"#;
    pub(crate) const FOREGROUND_TASK_DESCRIPTION: &str = "Sleep for 10 seconds";

    /// The same shape from the same probe run, for a foreground command that
    /// EXITED NON-ZERO: `sleep 3; exit 7`. Two task lines again, and the
    /// notification's status is `failed` — with no error text anywhere on it,
    /// which is why the row it mints names the work and stops there.
    pub(crate) const FOREGROUND_TASK_FAILED_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"boq8sla8p","tool_use_id":"toolu_fg2","description":"sleep 3; exit 7","task_type":"local_bash","uuid":"task-uuid-11","session_id":"sess-adk"}"#;
    pub(crate) const FOREGROUND_TASK_NOTIFICATION_FAILED: &str = r#"{"type":"system","subtype":"task_notification","task_id":"boq8sla8p","tool_use_id":"toolu_fg2","status":"failed","output_file":"","summary":"sleep 3; exit 7","uuid":"task-uuid-12","session_id":"sess-adk"}"#;
    pub(crate) const FOREGROUND_TASK_FAILED_DESCRIPTION: &str = "sleep 3; exit 7";

    /// A third terminal status the same probe run turned up, from a task the
    /// child killed when its turn ended: `stopped`. Recorded from that
    /// notification with its task id, `tool_use_id`, `summary` and
    /// `output_file` swapped for the completed foreground recording's, so one
    /// `task_started` line above serves this ending too. Its `status` — the one
    /// field this recording exists to pin — is verbatim.
    pub(crate) const FOREGROUND_TASK_NOTIFICATION_STOPPED: &str = r#"{"type":"system","subtype":"task_notification","task_id":"bwhwgc2zw","tool_use_id":"toolu_fg","status":"stopped","output_file":"","summary":"Sleep for 10 seconds","uuid":"task-uuid-13","session_id":"sess-adk"}"#;

    /// Which request the child names in the `control_response` it answers an
    /// interrupt with.
    enum Acknowledged {
        /// The request it was actually asked — what the live child does.
        TheOneAsked,
        /// Somebody else's, so the reader's matching is exercised rather than
        /// assumed: a response naming another request is noise, and the turn's
        /// own failure keeps its epitaph.
        AnotherRequest,
        /// The one asked, and NOTHING else: an interrupt that lands between
        /// turns has no turn to end, so no result follows the ack. What makes
        /// an interrupt recorded against a finished turn observable — the next
        /// result to arrive is then the NEXT turn's, and a session that took it
        /// as the stopped turn's would swallow that turn whole.
        WithNoTurnToStop,
    }

    /// A fake stream-json harness: it announces its session after a beat, then
    /// replays `per_turn` for every turn written to its stdin, turn after turn
    /// for as long as that stdin is open. The beat is what makes `Starting`
    /// observable — a real child's init line does not arrive the instant it is
    /// forked either.
    ///
    /// It also speaks the control protocol, in the order the live wire does: a
    /// `control_request` is answered with a `control_response`, then with the
    /// `error_during_execution` result that closes the turn it stopped, and
    /// only then does the loop read the next queued turn.
    pub(crate) fn stream_json_harness(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(INIT, &[per_turn], true, Acknowledged::TheOneAsked, None)
    }

    /// The same child, appending every line written to its stdin to `heard`.
    ///
    /// What Build SAID is otherwise invisible from outside the session, and the
    /// steering flow IS two writes in one order — so a test watching only what
    /// came back could not tell an interrupt that was sent from one that was
    /// not.
    pub(crate) fn stream_json_harness_recording_stdin(
        per_turn: &[&str],
        heard: &std::path::Path,
    ) -> HarnessSpec {
        harness_replaying(
            INIT,
            &[per_turn],
            true,
            Acknowledged::TheOneAsked,
            Some(heard),
        )
    }

    /// The same child, for one turn only: it answers, then leaves the way a
    /// real one does when its work is over. That departure closes its stream,
    /// which is what a no-terminal session's death rites hang off.
    pub(crate) fn stream_json_harness_that_leaves(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(INIT, &[per_turn], false, Acknowledged::TheOneAsked, None)
    }

    /// A CLI that announces no interrupt. Build never sends it a
    /// `control_request`, because [`AdkSession::can_interrupt`] reads the same
    /// announcement the refusal does.
    pub(crate) fn stream_json_harness_without_interrupt(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(
            INIT_WITHOUT_INTERRUPT,
            &[per_turn],
            true,
            Acknowledged::TheOneAsked,
            None,
        )
    }

    /// A child that answers an interrupt by naming a request nobody made, and
    /// then fails the turn on its own account.
    pub(crate) fn stream_json_harness_answering_another_request(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(INIT, &[per_turn], true, Acknowledged::AnotherRequest, None)
    }

    /// A recording child with nothing to stop: it acks a `control_request` and
    /// emits no result, because an interrupt that lands between turns ends no
    /// turn. Both halves matter to the stale press — the recorder says whether
    /// the press was spoken at all, and the missing result is what leaves the
    /// NEXT turn's result the only one there is.
    pub(crate) fn stream_json_harness_with_nothing_to_stop(
        per_turn: &[&str],
        heard: &std::path::Path,
    ) -> HarnessSpec {
        harness_replaying(
            INIT,
            &[per_turn],
            true,
            Acknowledged::WithNoTurnToStop,
            Some(heard),
        )
    }

    /// A child that answers each turn with its OWN recording — the first turn
    /// with the first, the second with the second, and every turn after that
    /// with the last one.
    ///
    /// What a single recording cannot express: work that is live after one turn
    /// and over after the next, which is the whole shape the idle sweep has to
    /// tell apart.
    pub(crate) fn stream_json_harness_turn_by_turn(turns: &[&[&str]]) -> HarnessSpec {
        harness_replaying(INIT, turns, true, Acknowledged::TheOneAsked, None)
    }

    fn harness_replaying(
        init: &str,
        turns: &[&[&str]],
        turn_after_turn: bool,
        acknowledged: Acknowledged,
        heard: Option<&std::path::Path>,
    ) -> HarnessSpec {
        let recorded = |lines: &[&str]| {
            let mut replay = String::new();
            for line in lines {
                assert!(
                    !line.contains('\''),
                    "a recorded protocol line may not carry a single quote: {line}"
                );
                replay.push_str(&format!("printf '%s\\n' '{line}'\n"));
            }
            replay
        };
        // One recording needs no counter and produces the script it always did.
        // Several are answered by a `case` on how many turns have been read, in
        // which the last recording is the catch-all — so a child asked for more
        // turns than were recorded keeps answering rather than falling silent.
        let (counter, replay) = match turns {
            [only] => (String::new(), recorded(only)),
            _ => {
                let mut arms = String::new();
                for (index, lines) in turns.iter().enumerate() {
                    let label = match index == turns.len() - 1 {
                        true => "*".to_string(),
                        false => (index + 1).to_string(),
                    };
                    arms.push_str(&format!("{label})\n{}\n;;\n", recorded(lines)));
                }
                (
                    "turns=0\n".to_string(),
                    format!("turns=$((turns+1))\ncase \"$turns\" in\n{arms}esac\n"),
                )
            }
        };
        // The one place the script interpolates rather than quoting a recording:
        // the id it echoes is a value it read at runtime.
        let echoed = match acknowledged {
            Acknowledged::TheOneAsked | Acknowledged::WithNoTurnToStop => "\"$asked\"",
            Acknowledged::AnotherRequest => "nobody-asked-this",
        };
        // The result that closes the turn the interrupt stopped — unless there
        // was no turn to stop, in which case the ack is the whole answer.
        let stopped_turn = match acknowledged {
            Acknowledged::WithNoTurnToStop => String::new(),
            _ => format!("printf '%s\\n' '{FAILED_RESULT}'\n"),
        };
        let mut script = format!("sleep 0.2\nprintf '%s\\n' '{init}'\n{counter}");
        script.push_str(match turn_after_turn {
            true => "while IFS= read -r turn; do\n",
            false => "if IFS= read -r turn; then\n",
        });
        if let Some(heard) = heard {
            script.push_str(&format!(
                "printf '%s\\n' \"$turn\" >> \"{}\"\n",
                heard.display()
            ));
        }
        script.push_str(&format!(
            "case \"$turn\" in\n\
             *control_request*)\n\
             asked=$(printf '%s' \"$turn\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             printf '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"%s\"}}}}\\n' {echoed}\n\
             {stopped_turn}\
             ;;\n\
             *)\n\
             {replay}\
             ;;\n\
             esac\n"
        ));
        script.push_str(match turn_after_turn {
            true => "done\n",
            false => "fi\n",
        });
        HarnessSpec::new("sh").arg("-c").arg(script)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn only_the_three_bad_endings_count_as_a_failure() {
        for failed in ["failed", "error", "timed_out"] {
            assert!(task_status_failed(failed), "{failed}");
            assert!(task_status_is_terminal(failed), "{failed}");
        }
        for ended in ["completed", "killed", "stopped", "cancelled", "canceled"] {
            assert!(!task_status_failed(ended), "{ended}");
            assert!(task_status_is_terminal(ended), "{ended}");
        }
    }

    fn open(spec: &HarnessSpec) -> AdkSession {
        AdkSession::spawn(spec, None)
            .expect("the fake harness spawns")
            .0
    }

    /// Wait for the session to report `want`, or fail saying what it reported
    /// instead. Statuses here are protocol-driven, so the wait is for a line to
    /// arrive rather than for a clock to run out.
    fn wait_for_status(session: &AdkSession, want: AgentStatus) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if session.status() == want {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "the session never reported {want:?} — it is {:?}",
            session.status()
        );
    }

    async fn next_activity(rx: &mut broadcast::Receiver<AgentActivity>) -> AgentActivity {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(activity)) => activity,
            Ok(Err(err)) => panic!("the activity stream ended before it reported: {err}"),
            Err(_) => panic!("no activity arrived within five seconds"),
        }
    }

    fn spawn_options() -> SpawnOptions {
        SpawnOptions {
            continue_session: false,
            resume_session_id: None,
            owner_id: "agent-01J".to_string(),
            mcp_session_token: "token-42".to_string(),
            cwd: PathBuf::from("/tmp/worktree"),
        }
    }

    fn context() -> HarnessContext {
        HarnessContext {
            bridge_exe: "/usr/local/bin/build-bridge".to_string(),
            mcp_socket: "/tmp/build-mcp.sock".to_string(),
        }
    }

    /// The launch config is the interactive one with the TUI swapped for the
    /// protocol: the same binary, the same permission grant, the same model
    /// flags, and the stream-json argv that makes a turn a value.
    #[test]
    fn the_headless_spec_runs_claude_over_stream_json_on_both_ends() {
        let choice = ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            model: Some("claude-fable-5".to_string()),
            effort: Some("high".to_string()),
        };
        let spec = AdkHarness.spec(&choice, &spawn_options(), &context());

        assert_eq!(spec.binary, "claude");
        let args = spec.args.join(" ");
        assert!(
            args.contains("-p --input-format stream-json --output-format stream-json --verbose"),
            "the protocol argv, whole and in order: {args}"
        );
        assert!(
            args.contains("--model claude-fable-5 --effort high"),
            "a model selection reaches the same flags claude has always taken: {args}"
        );
        assert!(args.contains("--dangerously-skip-permissions"), "{args}");
        assert!(
            !args.contains("--continue"),
            "nothing to continue was asked for: {args}"
        );
        assert_eq!(
            spec.submit_delay,
            Duration::ZERO,
            "a submit key is how a prompt is typed into a line editor, and there is none here"
        );

        let resumed = AdkHarness.spec(
            &choice,
            &SpawnOptions {
                continue_session: true,
                ..spawn_options()
            },
            &context(),
        );
        assert!(
            resumed.args.contains(&"--continue".to_string()),
            "a headless session picks the worktree's conversation back up: {:?}",
            resumed.args
        );
    }

    /// A conversation the last session NAMED is resumed by that name, and the
    /// cwd guess is not passed beside it: `--resume` names the exact
    /// conversation and `--continue` names the newest one in the directory, so
    /// asking for both is asking for two different conversations.
    #[test]
    fn a_recorded_session_id_is_resumed_by_name_instead_of_by_the_cwd_guess() {
        let by_name = AdkHarness.spec(
            &ModelChoice::default(),
            &SpawnOptions {
                // Both offered, exactly as the daemon offers them: the probe
                // answers for every Build-owned checkout, and the record
                // answers for an agent that has run before.
                continue_session: true,
                resume_session_id: Some("sess-adk".to_string()),
                ..spawn_options()
            },
            &context(),
        );
        let args = by_name.args.join(" ");
        assert!(args.contains("--resume sess-adk"), "{args}");
        assert!(
            !args.contains("--continue"),
            "the name wins, and it wins alone: {args}"
        );

        // And nothing recorded leaves the shipped fallback exactly as it was:
        // an agent whose session died before announcing itself must not be a
        // spawn that fails.
        let by_guess = AdkHarness.spec(
            &ModelChoice::default(),
            &SpawnOptions {
                continue_session: true,
                resume_session_id: None,
                ..spawn_options()
            },
            &context(),
        );
        let args = by_guess.args.join(" ");
        assert!(args.contains("--continue"), "{args}");
        assert!(!args.contains("--resume"), "{args}");
    }

    /// §2's dividend, held to: everything an agent says to Build arrives over
    /// the MCP socket, so the from-agent half of the interface needs zero work
    /// for a new carrier — provided the wiring really is identical. This is
    /// what checks that it is.
    #[test]
    fn the_headless_spec_carries_exactly_the_interactive_mcp_wiring() {
        let choice = ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            ..ModelChoice::default()
        };
        let options = spawn_options();
        let headless = AdkHarness.spec(&choice, &options, &context());
        let interactive = ClaudeHarness.spec(&ModelChoice::default(), &options, &context());

        assert_eq!(
            headless.env, interactive.env,
            "the same socket and the same per-process capability"
        );
        assert_eq!(
            headless.unset, interactive.unset,
            "an agent Build spawns is its own session on either carrier"
        );
        let mcp_args = |spec: &HarnessSpec| -> Vec<String> {
            spec.args
                .iter()
                .skip_while(|arg| *arg != "--mcp-config")
                .take(3)
                .cloned()
                .collect()
        };
        assert_eq!(
            mcp_args(&headless),
            mcp_args(&interactive),
            "the same per-agent config, loaded the same strict way"
        );
        assert!(
            mcp_args(&headless).contains(&crate::orchestrator::mcp_config_path(&options.owner_id))
        );
    }

    /// The catalog, the trust registry and the transcripts belong to the CLI,
    /// not to the carrier — it is the same claude, the same account and the
    /// same `~/.claude/projects`. Only the terminal answer differs.
    #[test]
    fn the_headless_provider_is_claude_in_every_way_but_its_carrier() {
        assert_eq!(
            AdkHarness.models().len(),
            ClaudeHarness.models().len(),
            "one catalog, one place to add a model"
        );
        assert_eq!(AdkHarness.effort_levels(), ClaudeHarness.effort_levels());

        let home = tempfile::tempdir().expect("temp home");
        let cwd = std::path::Path::new("/Users/z/proj");
        assert!(!AdkHarness.has_transcript(home.path(), cwd));
        let encoded = home
            .path()
            .join(".claude/projects")
            .join(crate::harness::claude::encode_project_dir(cwd));
        std::fs::create_dir_all(&encoded).expect("the transcript dir");
        std::fs::write(encoded.join("session.jsonl"), "{}\n").expect("a transcript");
        assert!(
            AdkHarness.has_transcript(home.path(), cwd),
            "a headless session writes the transcripts the TUI does, so a resume finds them"
        );

        assert!(
            !AdkHarness.has_terminal(),
            "and the one thing that does differ: no basement"
        );
    }

    /// The two capabilities are alternatives, and this carrier takes the second
    /// one: it reports its own reasoning and tool calls, so there is nothing for
    /// a human to escape to.
    #[test]
    fn a_reporting_session_has_no_terminal_and_offers_its_activity() {
        let session = open(&stream_json_harness(&[RESULT]));
        assert!(session.terminal().is_none());
        assert!(session.activity().is_some());
        session.end();
    }

    /// `Starting` is not a guess here. The child is forked long before it can
    /// take a turn, and the init line is the moment it can — so the status the
    /// PTY could never report is exactly what this carrier reads off the wire.
    #[test]
    fn status_is_starting_until_the_session_reports_init() {
        let session = open(&stream_json_harness(&[RESULT]));
        assert_eq!(
            session.status(),
            AgentStatus::Starting,
            "a forked child that has said nothing is not waiting for anyone"
        );
        wait_for_status(&session, AgentStatus::Waiting);
        assert_eq!(
            session.session_id().as_deref(),
            Some("sess-adk"),
            "the id a resume is passed comes from the init line"
        );
        session.end();
    }

    /// The turn boundary the whole carrier exists for: a model reasoning in
    /// silence is still working, and only its result line says otherwise.
    #[test]
    fn a_turn_is_working_until_its_result_line_arrives() {
        // This fake takes its time answering and says NOTHING while it does —
        // the silence the terminal carrier could only read as "waiting for you".
        let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'\nwhile IFS= read -r turn; do sleep 0.4; printf '%s\\n' '{RESULT}'; done\n"
        )));
        wait_for_status(&session, AgentStatus::Waiting);

        session
            .send_turn(&Turn::new("drop the index"))
            .expect("the turn is written");
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "the turn started the moment it was accepted"
        );
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "a silent model mid-turn is working, not waiting for the human"
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// A turn handed over mid-turn is absorbed by the one already running, and
    /// the child answers both with a single result. The session has to end that
    /// turn on it: a carrier that kept waiting for a second result would report
    /// `Working` forever, and `Working` is what stops the idle sweep from ever
    /// explaining an agent that quietly stopped.
    #[test]
    fn one_result_ends_the_turn_it_answers_however_many_were_handed_over() {
        // Two turns in, one result out, then silence — the shape of a message
        // written while the model was still working.
        let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'; read -r first; read -r second; printf '%s\\n' '{RESULT}'; cat >/dev/null"
        )));
        wait_for_status(&session, AgentStatus::Waiting);

        session.send_turn(&Turn::new("drop the index")).unwrap();
        session.send_turn(&Turn::new("and the trigger")).unwrap();
        assert_eq!(session.status(), AgentStatus::Working);

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// Every content block becomes one conversation event, in the order the
    /// child reported it — the timeline reads as the turn happened.
    #[tokio::test]
    async fn activity_is_minted_in_the_order_the_protocol_reported_it() {
        let session = open(&stream_json_harness(&[
            THINKING,
            TOOL_USE,
            TOOL_RESULT,
            NARRATION,
            RESULT,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("drop the index")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Reasoning {
                summary: "the index is unused".to_string()
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolUse {
                call_id: "toolu_1".to_string(),
                summary: "Read bridge/src/app.rs".to_string()
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_1".to_string(),
                outcome: ToolOutcome::Ok,
                summary: "fn main() {}".to_string()
            },
            "the answer names the call it answers, and carries the answer alone"
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            }
        );
        session.end();
    }

    /// The pairing is by id, never by adjacency: two calls answered in the
    /// reverse order each carry their own id, so the daemon lands each answer
    /// on the row its own call minted.
    #[tokio::test]
    async fn two_calls_answered_out_of_order_each_name_their_own_call() {
        let session = open(&stream_json_harness(&[
            TOOL_USE,
            SECOND_TOOL_USE,
            SECOND_TOOL_RESULT,
            TOOL_RESULT,
            RESULT,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("read both")).unwrap();

        for expected in ["toolu_1", "toolu_2"] {
            let AgentActivity::ToolUse { call_id, .. } = next_activity(&mut activity).await else {
                panic!("a call was expected");
            };
            assert_eq!(call_id, expected);
        }
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_2".to_string(),
                outcome: ToolOutcome::Ok,
                summary: "pub struct Thread".to_string()
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_1".to_string(),
                outcome: ToolOutcome::Ok,
                summary: "fn main() {}".to_string()
            }
        );
        session.end();
    }

    /// `is_error` is the whole of what says a call failed, and the failure
    /// travels as the outcome rather than as words in the summary.
    #[tokio::test]
    async fn a_failed_answer_reports_the_error_outcome_and_its_text() {
        let session = open(&stream_json_harness(&[TOOL_USE, ERROR_TOOL_RESULT, RESULT]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("read the file")).unwrap();

        assert!(matches!(
            next_activity(&mut activity).await,
            AgentActivity::ToolUse { .. }
        ));
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_1".to_string(),
                outcome: ToolOutcome::Error,
                summary: "File does not exist. Note: your current working directory is /work."
                    .to_string()
            }
        );
        session.end();
    }

    /// An answer to a call nobody announced still reaches the timeline: the
    /// reader knows nothing about it beyond its id and what it says, and says
    /// exactly that.
    #[tokio::test]
    async fn an_answer_to_a_call_nobody_announced_is_still_reported() {
        let session = open(&stream_json_harness(&[ORPHAN_TOOL_RESULT, RESULT]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("read the file")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_nobody_announced".to_string(),
                outcome: ToolOutcome::Ok,
                summary: "an answer to nothing".to_string()
            }
        );
        session.end();
    }

    /// The turn boundary drains the map. A call the interrupted turn left open
    /// is closed as unanswered — no answer ever arrived and none is coming —
    /// and the next turn's call pairs into a row of its own, which is what
    /// proves the drain emptied the map rather than leaving the id behind.
    #[tokio::test]
    async fn a_result_closes_every_call_its_turn_left_open() {
        let session = open(&stream_json_harness_turn_by_turn(&[
            &[TOOL_USE, FAILED_RESULT],
            &[SECOND_TOOL_USE, SECOND_TOOL_RESULT, RESULT],
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("read the file")).unwrap();

        assert!(matches!(
            next_activity(&mut activity).await,
            AgentActivity::ToolUse { .. }
        ));
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_1".to_string(),
                outcome: ToolOutcome::Unanswered,
                summary: String::new()
            },
            "the turn ended over the call, and nothing is fabricated about it"
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("try the other one")).unwrap();
        assert!(matches!(
            next_activity(&mut activity).await,
            AgentActivity::ToolUse { .. }
        ));
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                call_id: "toolu_2".to_string(),
                outcome: ToolOutcome::Ok,
                summary: "pub struct Thread".to_string()
            },
            "and the next turn starts against an empty map"
        );
        session.end();
    }

    /// A call reads as the thing it ran, and `Bash` runs a command. The
    /// `description` beside it is the model's paraphrase of the same act, and a
    /// row is one quiet line rather than two claims about it — so the command is
    /// what survives.
    #[test]
    fn a_bash_call_reads_as_the_command_it_ran() {
        assert_eq!(
            tool_call_summary(
                "Bash",
                &json!({ "command": "cargo test", "description": "Run the test suite" }),
            ),
            "Bash cargo test",
        );
    }

    /// Every tool the table names mints its own one human argument, whatever
    /// else the call carried beside it.
    #[test]
    fn each_named_tool_mints_the_argument_a_developer_would_read() {
        for (tool, input, want) in [
            (
                "Read",
                json!({ "file_path": "bridge/src/app.rs", "offset": 40 }),
                "Read bridge/src/app.rs",
            ),
            (
                "Write",
                json!({ "file_path": "spa/src/core/thread.js", "content": "export const x = 1;" }),
                "Write spa/src/core/thread.js",
            ),
            (
                "Edit",
                json!({ "file_path": "bridge/src/harness/adk.rs", "old_string": "a", "new_string": "b" }),
                "Edit bridge/src/harness/adk.rs",
            ),
            (
                "NotebookEdit",
                json!({ "notebook_path": "analysis.ipynb", "new_source": "print(1)" }),
                "NotebookEdit analysis.ipynb",
            ),
            (
                "Glob",
                json!({ "pattern": "**/*.rs", "path": "bridge" }),
                "Glob **/*.rs",
            ),
            (
                "Grep",
                json!({ "pattern": "tool_call_summary", "output_mode": "content" }),
                "Grep tool_call_summary",
            ),
            (
                "WebFetch",
                json!({ "url": "https://example.com/spec", "prompt": "what changed?" }),
                "WebFetch https://example.com/spec",
            ),
            (
                "WebSearch",
                json!({ "query": "stream-json protocol", "allowed_domains": ["example.com"] }),
                "WebSearch stream-json protocol",
            ),
            (
                "Task",
                json!({ "description": "audit the readers", "prompt": "read every reader" }),
                "Task audit the readers",
            ),
        ] {
            assert_eq!(tool_call_summary(tool, &input), want, "{tool}");
        }
    }

    /// A tool the table never heard of — an MCP tool, or one newer than the
    /// table — still reads as words rather than punctuation: the first
    /// string-valued field it carried, and never a brace.
    #[test]
    fn an_unlisted_tool_mints_its_first_string_field_and_no_braces() {
        let summary = tool_call_summary(
            "mcp__linear__create_issue",
            &json!({ "estimate": 3, "title": "the pump drops rows", "labels": ["bug"] }),
        );
        assert_eq!(summary, "mcp__linear__create_issue the pump drops rows");
        assert!(!summary.contains('{'), "{summary}");
    }

    /// With no words anywhere in the call, the tool's name is the whole row —
    /// and a listed tool whose meat key the call omitted falls through to the
    /// same fallback rather than inventing one.
    #[test]
    fn a_call_with_no_words_mints_the_tool_name_alone() {
        assert_eq!(tool_call_summary("Ping", &json!({ "attempts": 3 })), "Ping");
        assert_eq!(tool_call_summary("Bash", &json!({})), "Bash");
        assert_eq!(
            tool_call_summary("Read", &json!({ "offset": 10, "reason": "audit the pump" })),
            "Read audit the pump",
            "a listed tool without its own key takes the unlisted rule",
        );
    }

    /// The meat is clipped the way every summary is: one line, and the limit
    /// with an ellipsis behind it.
    #[test]
    fn a_long_command_clips_at_the_summary_limit() {
        let command = "x".repeat(500);
        assert_eq!(
            tool_call_summary("Bash", &json!({ "command": command })),
            format!("Bash {}…", "x".repeat(TOOL_SUMMARY_LIMIT)),
        );
    }

    /// The same rule through the pump the child actually speaks to: a recorded
    /// `Bash` call lands one row, and that row is the command line.
    #[tokio::test]
    async fn a_bash_call_reaches_the_conversation_as_its_command_line() {
        let session = open(&stream_json_harness(&[BASH_TOOL_USE, RESULT]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the suite")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolUse {
                call_id: "toolu_bash".to_string(),
                summary: "Bash cargo test".to_string(),
            }
        );
        session.end();
    }

    /// Every task row leads with the work it names and ends with what happened
    /// to it. A row carries no label in front of it any more, so a row that led
    /// with `started` would spend its first word on its least informative one —
    /// and the description is what a reader is scanning for.
    ///
    /// One leg for all four mints, because they are one wording: a task
    /// announcing itself, a patch that fails it, a roster that inserts it, a
    /// roster that drops it, and the notification row — which was
    /// description-first already and does not move.
    #[tokio::test]
    async fn task_rows_lead_with_the_work_and_end_with_what_happened() {
        let session = open(&stream_json_harness(&[
            TASK_STARTED,
            TASK_UPDATED_FAILED,
            TASK_ROSTER,
            TASK_ROSTER_EMPTY,
            TASK_STARTED,
            TASK_NOTIFICATION_MULTILINE,
            RESULT,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        for want in [
            format!("{TASK_DESCRIPTION} — started"),
            format!("{TASK_DESCRIPTION} — failed: exit code 1"),
            format!("{TASK_DESCRIPTION} — started"),
            format!("{TASK_DESCRIPTION} — finished"),
            format!("{TASK_DESCRIPTION} — started"),
            format!("{TASK_DESCRIPTION}: Background command completed woke"),
            format!("{TASK_DESCRIPTION} — finished"),
        ] {
            assert_eq!(
                next_activity(&mut activity).await,
                AgentActivity::TaskUpdate {
                    summary: want.clone()
                },
            );
        }
        session.end();
    }

    /// The row a background task mints when it starts, and the whole reason
    /// this step exists: the turn that started the work is over and the work is
    /// not, so a session with nothing open is still `Working`.
    ///
    /// The `background_tasks_changed` roster that follows lists the same task,
    /// and mints NOTHING — one row per transition, however many events describe
    /// it. The narration behind it is the fence that proves so: if the roster
    /// had minted, it would be sitting where the narration is.
    #[tokio::test]
    async fn a_started_task_mints_once_and_keeps_a_turnless_session_working() {
        let session = open(&stream_json_harness(&[
            TASK_STARTED,
            TASK_ROSTER,
            RESULT,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — started"),
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
            "the roster listing a task already live moved nothing, so it minted nothing"
        );

        // The narration arrived after the result, so the turn is closed — said
        // through the control the composer reads, which is tied to an open turn
        // and to nothing else.
        assert!(
            !session.can_interrupt(),
            "a background task is not a turn, and the interrupt stops a turn"
        );
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "the work outlived the turn that started it"
        );
        session.end();
    }

    /// The roster is the source of truth, in both directions: a task it stops
    /// listing is over, whether or not an event ever said so. The timeline never
    /// shows work that started and never ended, and the set clears rather than
    /// pinning `Working` forever.
    #[tokio::test]
    async fn a_roster_that_drops_a_task_closes_it_once_and_the_session_waits_again() {
        let session = open(&stream_json_harness(&[
            TASK_STARTED,
            RESULT,
            TASK_ROSTER_EMPTY,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — started"),
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — finished"),
            },
            "a roster that quietly drops a task still closes it in the timeline"
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
            "and closes it exactly once"
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// A `task_updated` that carries a terminal status ends the task itself —
    /// `failed` when it said so, with the error it named — and the roster that
    /// later omits the id mints nothing more, because by then nothing moves.
    #[tokio::test]
    async fn a_terminal_task_update_fails_the_task_once() {
        let session = open(&stream_json_harness(&[
            TASK_STARTED,
            TASK_UPDATED_FAILED,
            TASK_ROSTER_EMPTY,
            RESULT,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — started"),
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — failed: exit code 1"),
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
            "the roster that follows a task already ended moves nothing"
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// A notification is the task saying something worth reading, so its text is
    /// minted under the task's own name — collapsed onto one line, because this
    /// is operational text rather than the agent speaking — and, when the status
    /// it carries is terminal, it closes the task as well: the text first, then
    /// the ending it announces.
    ///
    /// The second notification lands with the set already empty, so it mints its
    /// text on its own and closes nothing: one row per transition, and by then
    /// nothing moves. A patch that moves neither membership nor any
    /// human-readable text is a progress counter ticking, and mints nothing at
    /// all.
    #[tokio::test]
    async fn a_task_notification_is_minted_and_a_progress_patch_is_not() {
        let session = open(&stream_json_harness(&[
            TASK_STARTED,
            TASK_UPDATED_PROGRESS,
            TASK_NOTIFICATION,
            TASK_NOTIFICATION_MULTILINE,
            RESULT,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — started"),
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!(
                    "{TASK_DESCRIPTION}: Background command \"{TASK_DESCRIPTION}\" completed (exit code 0)"
                ),
            },
            "the progress patch before it moved nothing and said nothing new"
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: format!("{TASK_DESCRIPTION} — finished"),
            },
            "the status it carried was terminal, so the notification ended the task"
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::TaskUpdate {
                summary: "Background command completed woke".to_string(),
            },
            "several lines of output are one row, the way a tool answer is — and the set no longer holds a name to speak it under"
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            }
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// A FOREGROUND Bash command, replayed in the order the live child emitted
    /// it: `task_started`, then a `task_notification` carrying a terminal
    /// status — and no roster and no `task_updated`, because the child sends
    /// neither for one.
    ///
    /// So the notification is the only event that says the work is over, and it
    /// has to close the task: a reader that took it for chatter would hold the
    /// task for the life of the session and report `Working` over an agent that
    /// has been idle for hours — the inverse of the failure this step exists to
    /// close. The `Waiting` at the end is that regression's fence.
    ///
    /// Its text mints nothing of its own here because a foreground
    /// notification's summary IS the task's description, and a row reading
    /// `Sleep for 10 seconds: Sleep for 10 seconds` says nothing the ending row
    /// did not.
    #[tokio::test]
    async fn a_foreground_tasks_notification_closes_it_and_the_session_waits_again() {
        let session = open(&stream_json_harness(&[
            FOREGROUND_TASK_STARTED,
            FOREGROUND_TASK_NOTIFICATION,
            RESULT,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session
            .send_turn(&Turn::new("sleep for ten seconds"))
            .unwrap();

        let mut minted = Vec::new();
        for _ in 0..3 {
            minted.push(next_activity(&mut activity).await);
        }
        assert_eq!(
            minted,
            vec![
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_DESCRIPTION} — started"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_DESCRIPTION} — finished"),
                },
                AgentActivity::Narration {
                    summary: "dropped the index".to_string()
                },
            ]
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// The other two terminal statuses a foreground notification carries, both
    /// live-recorded: `failed`, which reads as a failure and names no error
    /// because the notification carries none, and `stopped`, which does not —
    /// something ended that work, which is not the same thing to read.
    ///
    /// Both must remove. An unrecognised status would leave its task in the set
    /// with no roster coming to clear it, which is the same pin under a
    /// different name.
    #[tokio::test]
    async fn a_failed_foreground_notification_fails_and_a_stopped_one_finishes() {
        let session = open(&stream_json_harness(&[
            FOREGROUND_TASK_FAILED_STARTED,
            FOREGROUND_TASK_NOTIFICATION_FAILED,
            FOREGROUND_TASK_STARTED,
            FOREGROUND_TASK_NOTIFICATION_STOPPED,
            RESULT,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session
            .send_turn(&Turn::new("run the two commands"))
            .unwrap();

        let mut minted = Vec::new();
        for _ in 0..5 {
            minted.push(next_activity(&mut activity).await);
        }
        assert_eq!(
            minted,
            vec![
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_FAILED_DESCRIPTION} — started"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_FAILED_DESCRIPTION} — failed"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_DESCRIPTION} — started"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!("{FOREGROUND_TASK_DESCRIPTION} — finished"),
                },
                AgentActivity::Narration {
                    summary: "dropped the index".to_string()
                },
            ]
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// The recorded probe, replayed in the order the live child emitted it:
    /// roster, `task_started`, the turn's `result`, empty roster,
    /// `task_updated`, `task_notification`.
    ///
    /// Two of those six events move the set and four do not, so the timeline
    /// gets exactly three rows — the start, the end, and what the task said.
    /// The notification arrives after the roster already closed the task, which
    /// is why it reads as its own text rather than under a name the set no
    /// longer holds.
    #[tokio::test]
    async fn the_probes_own_order_mints_one_row_per_transition() {
        let session = open(&stream_json_harness(&[
            TASK_ROSTER,
            TASK_STARTED,
            RESULT,
            TASK_ROSTER_EMPTY,
            TASK_UPDATED_DONE,
            TASK_NOTIFICATION,
            NARRATION,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("run the reindex")).unwrap();

        let mut minted = Vec::new();
        for _ in 0..4 {
            minted.push(next_activity(&mut activity).await);
        }
        assert_eq!(
            minted,
            vec![
                AgentActivity::TaskUpdate {
                    summary: format!("{TASK_DESCRIPTION} — started"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!("{TASK_DESCRIPTION} — finished"),
                },
                AgentActivity::TaskUpdate {
                    summary: format!(
                        "Background command \"{TASK_DESCRIPTION}\" completed (exit code 0)"
                    ),
                },
                AgentActivity::Narration {
                    summary: "dropped the index".to_string()
                },
            ]
        );

        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
    }

    /// Build's own tools already arrive as themselves over the MCP socket —
    /// `done` posts a completion, `post_thread_message` posts a message. Minting
    /// the call as well would tell the timeline the same thing twice, so neither
    /// the call nor the answer to it is minted.
    #[tokio::test]
    async fn builds_own_tool_calls_are_not_minted() {
        let session = open(&stream_json_harness(&[
            DONE_CALL,
            DONE_RESULT,
            NARRATION,
            RESULT,
        ]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("finish up")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
            "the done call and its answer belong to the socket, not the timeline"
        );
        session.end();
    }

    /// A subagent's chatter is folded into the call that spawned it rather than
    /// minted beside it: one tool call the human can read, not a second
    /// conversation interleaved with the first.
    #[tokio::test]
    async fn subagent_events_are_folded_into_the_call_that_spawned_them() {
        let session = open(&stream_json_harness(&[SUBAGENT_TEXT, NARRATION, RESULT]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("delegate it")).unwrap();

        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            },
            "the subagent's own text is not a second voice in the conversation"
        );
        session.end();
    }

    /// The activity stream closing is what tells the daemon the session is over
    /// — the death rites hang off it the way they hang off a PTY's EOF.
    #[tokio::test]
    async fn the_activity_stream_closes_when_the_child_does() {
        let session = open(&HarnessSpec::new("sh").arg("-c").arg("exit 0"));
        let mut activity = session.activity().expect("a reporting session");
        let closed = tokio::time::timeout(Duration::from_secs(5), activity.recv()).await;
        assert!(
            matches!(closed, Ok(Err(broadcast::error::RecvError::Closed))),
            "a subscriber must observe the close, not hang: {closed:?}"
        );
        session.end();
    }

    /// The epitaph is REPORTED, never scraped: the last error the child said
    /// out loud is what explains the crash, because there is no screen to read.
    #[test]
    fn the_epitaph_is_the_error_the_session_reported() {
        let session = open(&stream_json_harness(&[FAILED_RESULT]));
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("try it")).unwrap();
        wait_for_status(&session, AgentStatus::Waiting);

        let deadline = Instant::now() + Duration::from_secs(5);
        while session.epitaph().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            session.epitaph().as_deref(),
            Some("the tool call was refused")
        );
        session.end();
    }

    /// A child that dies mid-turn never reports a result, so its last words are
    /// whatever it managed to say on stderr — the only surface left.
    #[test]
    fn a_child_that_dies_mid_turn_leaves_what_it_said_on_stderr() {
        let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'; read -r turn; echo 'API Error: overloaded_error' >&2; exit 7"
        )));
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("go")).unwrap();

        assert!(
            session.exited_within(Duration::from_secs(5)),
            "the child died mid-turn and never came back"
        );
        assert_eq!(session.status(), AgentStatus::Ended { code: Some(7) });
        assert_eq!(
            session.epitaph().as_deref(),
            Some("API Error: overloaded_error")
        );
        session.end();
    }

    /// The reap lag, from the other carrier's test: a dying child closes its
    /// pipes before its exit status is reapable, so the caller deciding whether
    /// a failed write means "crashed" rather than "wedged" waits it out here.
    #[test]
    fn exited_within_bridges_the_gap_until_the_exit_is_reapable() {
        let session = open(&HarnessSpec::new("sh").arg("-c").arg("sleep 0.15"));
        assert_eq!(
            session.status(),
            AgentStatus::Starting,
            "the child is still running, so nothing has ended"
        );
        assert!(
            session.exited_within(Duration::from_secs(5)),
            "the wait must outlast the lag between the pipes closing and the reap"
        );
        assert_eq!(session.status(), AgentStatus::Ended { code: Some(0) });
    }

    /// And a harness that keeps running is not declared dead by waiting.
    #[test]
    fn exited_within_gives_up_on_a_harness_that_keeps_running() {
        let session = open(&stream_json_harness(&[RESULT]));
        assert!(!session.exited_within(Duration::from_millis(50)));
        session.end();
    }

    /// Ending reaps: killing without collecting the status leaks one zombie per
    /// session on a daemon that never restarts.
    #[test]
    fn ending_a_session_reaps_the_child() {
        let session = open(&stream_json_harness(&[RESULT]));
        wait_for_status(&session, AgentStatus::Waiting);
        session.end();
        assert!(
            matches!(session.status(), AgentStatus::Ended { .. }),
            "the child is gone the moment the session ends"
        );
    }

    /// `send_turn` returns on the WRITE. The daemon's in-place nudge speaks from
    /// under the app-wide state lock, so a carrier that waited on the model here
    /// would stall every RPC, every pump and the idle sweep with it.
    #[test]
    fn send_turn_returns_on_the_write_even_when_the_child_never_answers() {
        let session = open(
            &HarnessSpec::new("sh")
                .arg("-c")
                .arg(format!("printf '%s\\n' '{INIT}'; cat >/dev/null")),
        );
        wait_for_status(&session, AgentStatus::Waiting);

        let started = Instant::now();
        session
            .send_turn(&Turn::new("this turn is never answered"))
            .expect("the write is accepted");
        let took = started.elapsed();

        assert!(
            took < Duration::from_millis(250),
            "handing over a turn took {took:?} — a caller holding the state lock would be stalled"
        );
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "an unanswered turn is still a turn in progress"
        );
        session.end();
    }

    /// The capability is the CHILD's answer, not the provider's: the same CLI
    /// advertises an interrupt on one version and not on the next, so the
    /// question is asked of the `init` line rather than of a version number.
    /// And it is offered only while there is a turn to spend it on, because
    /// what an interrupt ends is a turn.
    ///
    /// The equivalence between the flag and the call runs one way and is
    /// asserted as such: a refusal implies the flag is false, and a false flag
    /// with a turn open implies a refusal. Between turns the flag is false and
    /// the call is the satisfied no-op — the turn the human meant to stop is
    /// already over, which is an answer rather than an error.
    #[test]
    fn stopping_a_turn_is_offered_exactly_when_the_child_announced_it_and_a_turn_is_open() {
        let announced = open(&stream_json_harness(&[THINKING]));
        assert!(
            !announced.can_interrupt(),
            "a child that has said nothing has announced nothing"
        );
        assert!(
            matches!(announced.interrupt(), Err(HarnessError::Unsupported(_))),
            "and a refusal always means the flag was false"
        );

        wait_for_status(&announced, AgentStatus::Waiting);
        assert!(
            !announced.can_interrupt(),
            "announced, but idle at its prompt: there is no turn to stop"
        );
        assert!(
            announced.interrupt().is_ok(),
            "and the press that lands there is satisfied, not refused"
        );

        // A turn the child never answers, so it is still open to be stopped.
        announced.send_turn(&Turn::new("drop the index")).unwrap();
        assert!(announced.can_interrupt());
        assert!(announced.interrupt().is_ok());
        announced.end();

        let silent = open(&stream_json_harness_without_interrupt(&[THINKING]));
        wait_for_status(&silent, AgentStatus::Waiting);
        silent.send_turn(&Turn::new("drop the index")).unwrap();
        assert!(
            !silent.can_interrupt(),
            "a turn is open, so a false flag here is the carrier's refusal"
        );
        assert!(matches!(
            silent.interrupt(),
            Err(HarnessError::Unsupported(_))
        ));
        silent.end();
    }

    /// What an interrupt IS on the wire: one `control_request` line down the
    /// same pipe the turns go down, carrying a fresh id per ask — which is what
    /// lets the reader tell this session's ack from somebody else's.
    #[test]
    fn an_interrupt_is_one_control_request_line_with_an_id_of_its_own() {
        let dir = tempfile::tempdir().expect("temp dir");
        let capture = dir.path().join("stdin.jsonl");
        let session = open(&HarnessSpec::new("sh").arg("-c").arg(format!(
            "printf '%s\\n' '{INIT}'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> {}; done\n",
            capture.display()
        )));
        wait_for_status(&session, AgentStatus::Waiting);

        // A turn to stop: this child never answers one, so it stays open, and
        // an ask only travels while a turn is open.
        session.send_turn(&Turn::new("rewrite everything")).unwrap();
        session.interrupt().expect("this child advertised one");
        session.interrupt().expect("asking twice is allowed");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut written = Vec::new();
        while Instant::now() < deadline && written.len() < 3 {
            written = std::fs::read_to_string(&capture)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("a protocol line"))
                .collect();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            written.len(),
            3,
            "the turn, then one line per ask: {written:?}"
        );
        assert_eq!(written[0]["type"], "user", "{written:?}");
        let written = &written[1..];
        for asked in written {
            assert_eq!(asked["type"], "control_request");
            assert_eq!(asked["request"]["subtype"], "interrupt");
            assert!(
                asked["request_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "an ack can only be matched to a request that named itself: {asked}"
            );
        }
        assert_ne!(
            written[0]["request_id"], written[1]["request_id"],
            "a fresh id per request, so one ask's ack cannot close another's"
        );
        session.end();
    }

    /// A CLI built before the interrupt landed refuses, says what to do
    /// instead — the probes showed an ordinary message reaches the running turn
    /// at its next step boundary — and is the SAME session afterwards. A
    /// refusal is not a kill.
    #[test]
    fn a_child_that_advertises_no_interrupt_refuses_and_keeps_working() {
        let session = open(&stream_json_harness_without_interrupt(&[THINKING]));
        wait_for_status(&session, AgentStatus::Waiting);

        let refused = session
            .interrupt()
            .expect_err("this child cannot be stopped");
        let said = refused.to_string();
        assert!(
            said.contains("interrupt") && said.contains("message"),
            "the refusal names what is missing and where the human's words still land: {said}"
        );
        assert!(
            !said.contains("Esc"),
            "and never sends anyone to a terminal this carrier does not have: {said}"
        );

        session
            .send_turn(&Turn::new("carry on then"))
            .expect("the refusal left the session alive");
        assert_eq!(session.status(), AgentStatus::Working);
        session.end();
    }

    /// The rule the whole step exists to hold: a turn the human stopped leaves
    /// no epitaph. `error_during_execution` is what an interrupted turn's
    /// result carries, and reported as a crash it would end the human's own
    /// stop with a crash notice quoting it.
    #[test]
    fn an_acked_interrupt_leaves_the_turn_it_stopped_no_epitaph() {
        let session = open(&stream_json_harness(&[THINKING]));
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("rewrite everything")).unwrap();
        wait_for_status(&session, AgentStatus::Working);

        session.interrupt().expect("this child advertised one");
        wait_for_status(&session, AgentStatus::Waiting);

        assert_eq!(
            session.epitaph(),
            None,
            "the human stopped it — there is nothing to explain"
        );
        session.end();
    }

    /// The other two sides of the same equivalence, so the clearing can never
    /// be a blanket amnesty on `error_during_execution`.
    ///
    /// The ack is what makes an interrupt one the child ACTED on: it answers
    /// the control request before it emits the result, so an interrupt still
    /// unanswered at the result is one the child never acted on, and the
    /// failure the result reports is the turn's own. A `control_response`
    /// carrying somebody else's request id is noise, and a session that treated
    /// it as its own would swallow a real crash.
    #[test]
    fn an_interrupt_the_child_never_answered_leaves_the_turns_own_error() {
        let session = open(&stream_json_harness_answering_another_request(&[THINKING]));
        wait_for_status(&session, AgentStatus::Waiting);
        session.send_turn(&Turn::new("rewrite everything")).unwrap();
        wait_for_status(&session, AgentStatus::Working);

        session.interrupt().expect("this child advertised one");
        wait_for_status(&session, AgentStatus::Waiting);

        assert_eq!(
            session.epitaph().as_deref(),
            Some("the tool call was refused"),
            "an interrupt the child never acted on does not excuse the turn's own failure"
        );
        session.end();
    }

    /// A press that lands after the turn it meant to stop has already ended.
    ///
    /// The control is offered off a digest up to 1.6s old, so a result can land
    /// inside that window — or race the ask by milliseconds. The rule is the
    /// one the take-on-result holds: an interrupt can never leak into the turn
    /// AFTER it, and taking the pending on a result only holds that when a
    /// result intervenes. Recorded against a turn already closed, the pending
    /// would be marked `steered` by the send that follows, and that turn's OWN
    /// result would then hand `Working` to nothing — a session reporting
    /// `Working` with nothing running, which the idle sweep will not demote,
    /// since it demotes only what is not working — while clearing that turn's
    /// own error from the epitaph.
    #[test]
    fn a_press_that_lands_between_turns_leaves_the_next_turn_alone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let heard = dir.path().join("heard.jsonl");
        let session = open(&stream_json_harness_with_nothing_to_stop(
            &[FAILED_RESULT],
            &heard,
        ));
        wait_for_status(&session, AgentStatus::Waiting);

        // A turn runs and its result closes it — the window the press lands in.
        session.send_turn(&Turn::new("drop the index")).unwrap();
        wait_for_status(&session, AgentStatus::Waiting);

        // The human presses stop on that turn, a moment too late, and the
        // message rides along the way the composer sends it.
        session.interrupt().expect("this child advertised one");
        session.send_turn(&Turn::new("try the other file")).unwrap();
        assert_eq!(session.status(), AgentStatus::Working);

        wait_for_status(&session, AgentStatus::Waiting);
        assert_eq!(
            session.epitaph().as_deref(),
            Some("the tool call was refused"),
            "the new turn failed on its own account; no interrupt of an older turn excuses it"
        );

        // And the child was never told to stop a turn it had already finished.
        // Not merely tidy: the live CLI announces `interrupt_cancel_queued_v1`,
        // so a `control_request` sent with nothing running is a request that
        // could take the queued turn with it.
        let kinds: Vec<String> = std::fs::read_to_string(&heard)
            .expect("the child kept what it heard")
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("a protocol line")["type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["user", "user"],
            "a press with no turn open is not spoken to the child at all"
        );
        session.end();
    }

    /// The subtler half of the rule: the result that closes an interrupted turn
    /// hands `Working` on to the turn queued behind it.
    ///
    /// Clearing the flag on that result would leave a session that is actively
    /// running the steering turn reporting `Waiting` — and a steering turn is
    /// exactly the kind that goes silent for minutes inside one tool call, so
    /// the idle sweep would demote a working agent. There is no second result
    /// to reopen it: the steering turn ends in its own single result.
    #[tokio::test]
    async fn a_steering_turn_behind_an_interrupt_keeps_the_session_working() {
        let session = open(&stream_json_harness(&[THINKING]));
        let mut activity = session.activity().expect("a reporting session");
        wait_for_status(&session, AgentStatus::Waiting);

        session.send_turn(&Turn::new("rewrite everything")).unwrap();
        next_activity(&mut activity).await;

        // In the order the daemon's steering flow speaks them, both from under
        // the state lock: stop the turn, then hand over the message.
        session.interrupt().expect("this child advertised one");
        session
            .send_turn(&Turn::new("actually, just the index"))
            .unwrap();

        // The child answers the interrupt, ends the stopped turn with
        // `error_during_execution`, and only then reads the steering turn — so
        // this second event can only arrive after that result was read.
        next_activity(&mut activity).await;
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "the steered turn is running; a session reporting Waiting here would be swept"
        );
        assert_eq!(session.epitaph(), None, "and the stop left no epitaph");
        session.end();
    }

    /// The quiet clock is the minutes-scale anomaly instrument the idle sweep
    /// demotes on, and for this carrier it reads protocol lines rather than
    /// paint — the last thing the session actually said.
    #[test]
    fn quiet_for_reads_the_age_of_the_last_protocol_line() {
        let session = open(&stream_json_harness(&[RESULT]));
        wait_for_status(&session, AgentStatus::Waiting);
        assert!(
            session.quiet_for() < Duration::from_secs(1),
            "the init line just arrived"
        );

        session.backdate_last_output(Duration::from_secs(600));
        assert!(
            session.quiet_for() >= Duration::from_secs(600),
            "a session that has said nothing for ten minutes must report it"
        );
        session.end();
    }

    /// The one claim the fake cannot prove: the live protocol's mid-turn
    /// semantics. Spawns the REAL `claude` binary with the REAL argv shape and
    /// hands it a turn that runs a slow tool; while that tool runs, a second
    /// turn is written. If streaming input delivers at the next step boundary
    /// — the way the interactive TUI queues a message typed mid-run — the
    /// agent's final answer obeys the follow-up inside the same turn. If the
    /// follow-up instead waits for the first result, a second Working phase
    /// appears and the answer still converges, but the printout says which
    /// world we are in.
    ///
    /// Ignored by default: it needs `claude` installed, authenticated, and a
    /// real (small, haiku) model turn. Run by hand:
    ///
    /// ```text
    /// cargo test --lib real_adk -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
    fn real_adk_session_steers_mid_turn() {
        use crate::harness::Harness;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("fresh-worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        let mcp = workspace.join("mcp.json");
        std::fs::write(
            &mcp,
            serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
        )
        .unwrap();
        AdkHarness.prepare_workspace(&workspace);

        let spec = HarnessSpec::new("claude")
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--mcp-config")
            .arg(mcp.to_string_lossy())
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg("haiku");

        let (session, mut activity) =
            AdkSession::spawn(&spec, Some(workspace.clone())).expect("claude should spawn");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let seen = std::sync::Arc::clone(&seen);
            std::thread::spawn(move || {
                while let Ok(event) = activity.blocking_recv() {
                    let line = match &event {
                        AgentActivity::Reasoning { summary } => format!("reasoning: {summary}"),
                        AgentActivity::ToolUse { summary, .. } => format!("tool_use: {summary}"),
                        AgentActivity::ToolResult {
                            outcome, summary, ..
                        } => format!("tool_result[{outcome:?}]: {summary}"),
                        AgentActivity::Narration { summary } => format!("narration: {summary}"),
                        AgentActivity::TaskUpdate { summary } => format!("task_update: {summary}"),
                    };
                    eprintln!("[activity] {line}");
                    seen.lock().unwrap().push(line);
                }
            });
        }

        let wait_until = |what: &str, deadline: Duration, test: &dyn Fn() -> bool| {
            let started = Instant::now();
            while !test() {
                assert!(started.elapsed() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(100));
            }
        };

        // The real CLI announces itself only after the first stdin message
        // arrives (verified against 2.1.236), so the turn is written first and
        // init is awaited after — the same order the daemon's deliver uses.
        session
            .send_turn(&Turn::new(concat!(
                "You are being driven by an automated test. Do exactly this and nothing ",
                "else, then stop. First, use the Bash tool to run exactly: sleep 10\n",
                "After the sleep finishes, write a file named answer.txt in the current ",
                "directory whose entire contents are exactly the single word: APPLE",
            )))
            .unwrap();
        wait_until("init", Duration::from_secs(30), &|| {
            !matches!(session.status(), AgentStatus::Starting)
        });
        wait_until("the sleep tool to start", Duration::from_secs(90), &|| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("tool_use") && line.contains("sleep"))
        });

        // Not immediately on the tool_use line: the probes show a message
        // written within ~100ms of the tool_use event can be lost in the CLI's
        // loop transition, while one written seconds later — any human
        // follow-up — is delivered at the next step boundary.
        std::thread::sleep(Duration::from_secs(3));
        let steered_at = Instant::now();
        session
            .send_turn(&Turn::new(concat!(
                "Change of plan: answer.txt must contain exactly the single word BANANA ",
                "instead of APPLE. This message supersedes the previous instruction.",
            )))
            .unwrap();
        eprintln!("[steer] follow-up written while the sleep tool runs");

        wait_until("the turn to end", Duration::from_secs(240), &|| {
            matches!(session.status(), AgentStatus::Waiting)
        });
        let first_result_after = steered_at.elapsed();

        // A second Working phase here would mean the follow-up was NOT absorbed
        // into the running turn and ran as its own turn after the first result.
        let mut second_turn = false;
        let settled = Instant::now();
        while settled.elapsed() < Duration::from_secs(20) {
            if matches!(session.status(), AgentStatus::Working) {
                second_turn = true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        wait_until("any second turn to end", Duration::from_secs(240), &|| {
            matches!(session.status(), AgentStatus::Waiting)
        });

        let answer = std::fs::read_to_string(workspace.join("answer.txt"))
            .expect("the agent should have written answer.txt");
        eprintln!(
            "[verdict] answer.txt = {:?}; first result {}ms after steering; second turn: {}",
            answer.trim(),
            first_result_after.as_millis(),
            second_turn,
        );
        assert_eq!(
            answer.trim(),
            "BANANA",
            "the mid-turn follow-up must decide the answer (same turn or the very next)"
        );
        assert!(
            !second_turn,
            "the follow-up ran as a separate turn after the first result — mid-turn steering does NOT reach the running loop; the spec's claim needs revising"
        );
        session.end();
    }

    /// The other live claim: the native interrupt, against the REAL wire. While
    /// a real turn sits inside a slow tool, `interrupt()` writes the
    /// `control_request`; the live child must ack it, close the stopped turn
    /// with `error_during_execution`, and run the steering turn queued behind
    /// the interrupt in the SAME session. Three things only the real binary can
    /// prove: the ack arrives (the epitaph clearing hangs on it — an unacked
    /// interrupt would leave `error_during_execution` reading as a crash), the
    /// stopped tool never finishes, and the conversation survives its own stop.
    ///
    /// The parked tool is a python sleep rather than a plain `sleep 90`
    /// because the installed CLI BLOCKS a standalone sleep outright — "Blocked:
    /// standalone sleep 90 … use run_in_background" — and the model, told no,
    /// obligingly reruns it in the background, where nothing is parked at all
    /// and the turn ends immediately. This command the CLI runs in the
    /// foreground, timeout and all, which is what parks the turn (verified on
    /// the wire, 2026-08-30: tool call at 9s, still running at 12s, and the
    /// interrupt cut it there).
    ///
    /// Ignored by default for the same reason as the steering test above; run
    /// with the same `cargo test --lib real_adk -- --ignored --nocapture`.
    #[test]
    #[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
    fn real_adk_session_interrupts_mid_tool() {
        use crate::harness::Harness;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("fresh-worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        let mcp = workspace.join("mcp.json");
        std::fs::write(
            &mcp,
            serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
        )
        .unwrap();
        AdkHarness.prepare_workspace(&workspace);

        let spec = HarnessSpec::new("claude")
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--mcp-config")
            .arg(mcp.to_string_lossy())
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg("haiku");

        let (session, mut activity) =
            AdkSession::spawn(&spec, Some(workspace.clone())).expect("claude should spawn");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let seen = std::sync::Arc::clone(&seen);
            std::thread::spawn(move || {
                while let Ok(event) = activity.blocking_recv() {
                    let line = match &event {
                        AgentActivity::Reasoning { summary } => format!("reasoning: {summary}"),
                        AgentActivity::ToolUse { summary, .. } => format!("tool_use: {summary}"),
                        AgentActivity::ToolResult {
                            outcome, summary, ..
                        } => format!("tool_result[{outcome:?}]: {summary}"),
                        AgentActivity::Narration { summary } => format!("narration: {summary}"),
                        AgentActivity::TaskUpdate { summary } => format!("task_update: {summary}"),
                    };
                    eprintln!("[activity] {line}");
                    seen.lock().unwrap().push(line);
                }
            });
        }

        let wait_until = |what: &str, deadline: Duration, test: &dyn Fn() -> bool| {
            let started = Instant::now();
            while !test() {
                assert!(started.elapsed() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(100));
            }
        };

        // A turn that parks inside a tool long enough to be stopped: if the
        // interrupt were silently ignored, this command runs its full ninety
        // seconds and the deadline math below catches it.
        session
            .send_turn(&Turn::new(concat!(
                "You are being driven by an automated test. Do exactly this and nothing ",
                "else, then stop. First, use the Bash tool to run exactly this command ",
                "in the FOREGROUND, and do NOT set run_in_background: ",
                "python3 -c \"import time; time.sleep(90)\"\n",
                "Set the tool timeout to 150000. After it finishes, write a file named ",
                "answer.txt in the current directory whose entire contents are exactly ",
                "the single word: APPLE",
            )))
            .unwrap();
        // The turn just written holds status at `Working`, so init's arrival is
        // observed through the capability it carries: `can_interrupt()` turns
        // true the moment the child's own line announces
        // `interrupt_receipt_v1`. A timeout here means the live CLI stopped
        // advertising it — and then this leg proves nothing.
        wait_until(
            "the child to announce its interrupt",
            Duration::from_secs(30),
            &|| session.can_interrupt(),
        );
        wait_until("the sleep tool to start", Duration::from_secs(90), &|| {
            seen.lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("tool_use") && line.contains("time.sleep"))
        });

        // The same berth the steering test gives the CLI's loop transition.
        std::thread::sleep(Duration::from_secs(3));
        let interrupted_at = Instant::now();
        session.interrupt().expect("the child advertised one");
        // The daemon's steering order, from `nudge_live_agent_tab`: stop, then
        // hand over. The steering message names nothing the first one did not,
        // so the right file appearing is also proof the conversation survived.
        session
            .send_turn(&Turn::new(concat!(
                "You were interrupted on purpose; that is expected. Do not sleep again. ",
                "Write the same file the first message named, but its entire contents ",
                "must be exactly the single word: CHERRY. Then stop.",
            )))
            .unwrap();
        eprintln!("[interrupt] control_request written mid-sleep, steering turn queued behind it");

        // Status must hold `Working` across the interrupted turn's result — the
        // steered hand-off — so `Waiting` here means the steering turn ended in
        // its own result.
        wait_until(
            "the steering turn to end",
            Duration::from_secs(240),
            &|| matches!(session.status(), AgentStatus::Waiting),
        );
        let settled_after = interrupted_at.elapsed();

        let answer = std::fs::read_to_string(workspace.join("answer.txt"))
            .expect("the steering turn should have written answer.txt");
        eprintln!(
            "[verdict] answer.txt = {:?}; settled {}ms after the interrupt; epitaph: {:?}",
            answer.trim(),
            settled_after.as_millis(),
            session.epitaph(),
        );
        // The stopped command still had some eighty-seven of its ninety seconds
        // to run, so an interrupt the child ignored cannot settle inside this
        // window however fast the steering turn is. On the wire the whole
        // stop-and-steer takes about five seconds.
        assert!(
            settled_after < Duration::from_secs(60),
            "the whole stop-and-steer took {}ms — the parked command had eighty-seven seconds left, so the turn was never stopped",
            settled_after.as_millis()
        );
        assert_eq!(
            answer.trim(),
            "CHERRY",
            "the steering turn must decide the file, in the same conversation"
        );
        assert_eq!(
            session.epitaph(),
            None,
            "the human stopped it — an epitaph here means the ack was missed and error_during_execution read as a crash"
        );
        assert!(
            !matches!(session.status(), AgentStatus::Ended { .. }),
            "an interrupted session is the same session, still alive"
        );
        // Step 12's half of this leg: the parked call must not outlive the
        // turn the interrupt ended — every call the session made closes. On
        // the live wire (claude 2.1.x, observed 2026-08-30) the CLI answers
        // the interrupted call ITSELF, with an `is_error` rejection ("The user
        // doesn't want to proceed…"), before the `error_during_execution`
        // result — so the boundary drain finds the map already empty. The
        // drain stays as the net beneath a wire that does not answer (a
        // crashed child, an older CLI), pinned by the fake in
        // `a_result_closes_every_call_its_turn_left_open`; what the live leg
        // holds is the invariant both paths serve: one completion per call,
        // and the interrupted call's completion is terminal — `Error` from the
        // CLI's own rejection, or `Unanswered` from the drain.
        let lines = seen.lock().unwrap().clone();
        let calls = lines
            .iter()
            .filter(|line| line.starts_with("tool_use:"))
            .count();
        let completions = lines
            .iter()
            .filter(|line| line.starts_with("tool_result["))
            .count();
        assert_eq!(
            completions, calls,
            "every call closes at its turn's boundary, the interrupted one included: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("tool_result[Error]")
                    || line.starts_with("tool_result[Unanswered]")),
            "the interrupted call's completion is terminal, never a fabricated success: {lines:?}"
        );
        session.end();
    }

    /// Step 11's live claim: a REAL background task, on the real wire. The
    /// recorded fixtures pin what one probe emitted; this leg holds the shipped
    /// reader to a fresh child — the task events still arrive on `system`, the
    /// reader mints the started and finished rows, and the session reports
    /// `Working` with `can_interrupt` false while its turn is closed and the
    /// task lives, then `Waiting` once the roster empties.
    ///
    /// Ignored by default for the same reason as the two legs above; run with
    /// the same `cargo test --lib real_adk -- --ignored --nocapture`.
    #[test]
    #[ignore = "spawns the real claude binary; needs auth + network + a model turn"]
    fn real_adk_session_reports_a_background_task_and_stays_working() {
        use crate::harness::Harness;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("fresh-worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        let mcp = workspace.join("mcp.json");
        std::fs::write(
            &mcp,
            serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
        )
        .unwrap();
        AdkHarness.prepare_workspace(&workspace);

        let spec = HarnessSpec::new("claude")
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--mcp-config")
            .arg(mcp.to_string_lossy())
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions")
            .arg("--model")
            .arg("haiku");

        let (session, mut activity) =
            AdkSession::spawn(&spec, Some(workspace.clone())).expect("claude should spawn");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let seen = std::sync::Arc::clone(&seen);
            std::thread::spawn(move || {
                while let Ok(event) = activity.blocking_recv() {
                    let line = match &event {
                        AgentActivity::Reasoning { summary } => format!("reasoning: {summary}"),
                        AgentActivity::ToolUse { summary, .. } => format!("tool_use: {summary}"),
                        AgentActivity::ToolResult {
                            outcome, summary, ..
                        } => format!("tool_result[{outcome:?}]: {summary}"),
                        AgentActivity::Narration { summary } => format!("narration: {summary}"),
                        AgentActivity::TaskUpdate { summary } => format!("task_update: {summary}"),
                    };
                    eprintln!("[activity] {line}");
                    seen.lock().unwrap().push(line);
                }
            });
        }

        let wait_until = |what: &str, deadline: Duration, test: &dyn Fn() -> bool| {
            let started = Instant::now();
            while !test() {
                assert!(started.elapsed() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        // A task row names its work first and what happened to it after, and a
        // failed one carries the error behind that — so the ending is looked
        // for anywhere in the line, with the `task_update:` guard kept.
        let task_rows = |marker: &str| {
            seen.lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with("task_update: ") && line.contains(marker))
                .count()
        };

        // A turn that puts a sleep in the BACKGROUND and answers without
        // waiting on it, so the turn's result closes over live work — the exact
        // shape the headless-looks-idle finding described.
        session
            .send_turn(&Turn::new(concat!(
                "You are being driven by an automated test. Do exactly this and nothing ",
                "else. Use the Bash tool with run_in_background set to true to run ",
                "exactly: sleep 15 && echo woke\n",
                "Do NOT wait for it, do NOT check on it, do NOT use any other tool. ",
                "Immediately after starting it, reply with a two-line haiku and stop.",
            )))
            .unwrap();
        wait_until("init", Duration::from_secs(30), &|| {
            !matches!(session.status(), AgentStatus::Starting)
        });
        wait_until("the started task row", Duration::from_secs(120), &|| {
            task_rows(" — started") == 1
        });

        // The result must close the turn while the task lives. The turn's edge
        // is read through the control tied to it: `can_interrupt` goes false
        // when the turn closes, while the live task holds status at `Working` —
        // the legal pair the digest pins, observed on the real wire.
        wait_until(
            "the turn to close over the live task",
            Duration::from_secs(120),
            &|| !session.can_interrupt(),
        );
        assert_eq!(
            session.status(),
            AgentStatus::Working,
            "the turn is closed and the sleep is not: this session is mid-work, not idle"
        );

        // The sleep ends; the roster empties; the reader closes the task in the
        // timeline and the session finally waits.
        wait_until(
            "the task to finish in the timeline",
            Duration::from_secs(120),
            &|| task_rows(" — finished") + task_rows(" — failed") >= 1,
        );
        wait_until(
            "the session to wait once the roster empties",
            Duration::from_secs(60),
            &|| matches!(session.status(), AgentStatus::Waiting),
        );
        eprintln!(
            "[verdict] task rows: {:?}",
            seen.lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with("task_update:"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            task_rows(" — started"),
            1,
            "one start, one row — however many events described it"
        );
        assert_eq!(
            task_rows(" — finished"),
            1,
            "the task completed, so its ending reads as finished, minted once"
        );
        session.end();
    }
}
