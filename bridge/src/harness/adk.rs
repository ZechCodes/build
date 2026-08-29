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

use std::collections::HashMap;
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
    AgentActivity, AgentSession, AgentStatus, Harness, HarnessContext, HarnessError, Turn,
    INHERITED_AGENT_MARKERS,
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

    fn label(&self) -> &'static str {
        "Claude Code (headless)"
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
        }
    }

    /// What a session that has not exited is doing, straight from its own turn
    /// boundaries: starting until it announces itself, working while a turn it
    /// accepted is unanswered, waiting for the human otherwise.
    fn live_status(&self) -> AgentStatus {
        if self.turn_open {
            AgentStatus::Working
        } else if self.announced {
            AgentStatus::Waiting
        } else {
            AgentStatus::Starting
        }
    }
}

/// What became of one tool call, kept until its result arrives so the answer can
/// be named — or dropped, when the call itself was Build's own.
enum RecordedCall {
    Minted { tool: String },
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
    /// answers differently on two versions of the same CLI — and a session that
    /// has not announced yet answers no, which is also true: it has no turn to
    /// stop.
    fn can_interrupt(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .capabilities
            .iter()
            .any(|announced| announced == INTERRUPT_CAPABILITY)
    }

    /// One `control_request` line on the same pipe the turns go down, and back.
    ///
    /// No wait for the ack — the contract is `send_turn`'s, for `send_turn`'s
    /// reason. What the ack decides is read later, by the reader thread, when
    /// the result that closes the stopped turn arrives.
    ///
    /// A second ask while one is outstanding replaces it: asking twice to stop
    /// the same turn is one ask.
    fn interrupt(&self) -> Result<(), HarnessError> {
        if !self.can_interrupt() {
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
        // Recorded before the write rather than after it: the child can answer
        // faster than this thread reaches its next lock, and an ack that
        // arrived before the record existed would read as somebody else's.
        self.state.lock().unwrap().pending_interrupt = Some(PendingInterrupt {
            request_id,
            acked: false,
            steered: false,
        });
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

    /// The lifecycle line. `init` is when the child can take a turn, and it
    /// carries the session id a respawn resumes by.
    fn read_system(&mut self, event: &Value) {
        if event["subtype"].as_str() != Some("init") {
            return;
        }
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
        let mut state = self.state.lock().unwrap();
        // Taken, acked or not, so an interrupt can never leak into the turn
        // after the one it ended.
        let stopped = state.pending_interrupt.take();
        // The turn queued behind an interrupt is running the moment this result
        // lands, so the flag is handed to it rather than cleared.
        state.turn_open = stopped.as_ref().is_some_and(|pending| pending.steered);
        state.reported_error =
            match failed && !stopped.as_ref().is_some_and(|pending| pending.acked) {
                true => Some(result_error_text(event)),
                false => None,
            };
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
        let id = block["id"].as_str().unwrap_or_default().to_string();
        if tool.starts_with(BUILD_MCP_TOOL_PREFIX) {
            self.calls.insert(id, RecordedCall::BuildsOwn);
            return;
        }
        let summary = tool_call_summary(&tool, &block["input"]);
        self.calls.insert(id, RecordedCall::Minted { tool });
        self.emit(AgentActivity::ToolUse { summary });
    }

    fn read_tool_result(&mut self, block: &Value) {
        let id = block["tool_use_id"].as_str().unwrap_or_default();
        // Taken, not read: a call is answered once, and a session that runs for
        // hours must not accumulate one entry per tool call it ever made.
        let call = self.calls.remove(id);
        if matches!(call, Some(RecordedCall::BuildsOwn)) {
            return;
        }
        let answer = one_line(&tool_result_text(block), TOOL_SUMMARY_LIMIT);
        let summary = match call {
            Some(RecordedCall::Minted { tool }) if !answer.is_empty() => {
                format!("{tool}: {answer}")
            }
            Some(RecordedCall::Minted { tool }) => tool,
            _ => answer,
        };
        if !summary.is_empty() {
            self.emit(AgentActivity::ToolResult { summary });
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

/// A tool call on one line: the tool's name, plus its input.
///
/// A call with a single string argument reads as that argument — `Read
/// bridge/src/app.rs` — because that is the call as a human would say it.
/// Anything else keeps its shape as compact JSON rather than being guessed at:
/// a summary that silently picked one of three arguments would be a different
/// call than the one that ran.
fn tool_call_summary(tool: &str, input: &Value) -> String {
    let rendered = match input.as_object() {
        Some(fields) => match fields.values().next() {
            Some(Value::String(only)) if fields.len() == 1 => only.clone(),
            _ => input.to_string(),
        },
        None => match input {
            Value::Null => String::new(),
            other => other.to_string(),
        },
    };
    let rendered = one_line(&rendered, TOOL_SUMMARY_LIMIT);
    match rendered.is_empty() {
        true => tool.to_string(),
        false => format!("{tool} {rendered}"),
    }
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
    pub(crate) const NARRATION: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"dropped the index"}]},"parent_tool_use_id":null}"#;
    pub(crate) const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"dropped the index","session_id":"sess-adk"}"#;
    pub(crate) const DONE_CALL: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_done","name":"mcp__build__done","input":{"phase":"build","status":"completed"}}]},"parent_tool_use_id":null}"#;
    pub(crate) const DONE_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_done","content":"recorded"}]},"parent_tool_use_id":null}"#;
    pub(crate) const SUBAGENT_TEXT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a subagent talking"}]},"parent_tool_use_id":"toolu_1"}"#;
    pub(crate) const FAILED_RESULT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"the tool call was refused","session_id":"sess-adk"}"#;

    /// Which request the child names in the `control_response` it answers an
    /// interrupt with.
    enum Acknowledged {
        /// The request it was actually asked — what the live child does.
        TheOneAsked,
        /// Somebody else's, so the reader's matching is exercised rather than
        /// assumed: a response naming another request is noise, and the turn's
        /// own failure keeps its epitaph.
        AnotherRequest,
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
        harness_replaying(INIT, per_turn, true, Acknowledged::TheOneAsked, None)
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
        harness_replaying(INIT, per_turn, true, Acknowledged::TheOneAsked, Some(heard))
    }

    /// The same child, for one turn only: it answers, then leaves the way a
    /// real one does when its work is over. That departure closes its stream,
    /// which is what a no-terminal session's death rites hang off.
    pub(crate) fn stream_json_harness_that_leaves(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(INIT, per_turn, false, Acknowledged::TheOneAsked, None)
    }

    /// A CLI that announces no interrupt. Build never sends it a
    /// `control_request`, because [`AdkSession::can_interrupt`] reads the same
    /// announcement the refusal does.
    pub(crate) fn stream_json_harness_without_interrupt(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(
            INIT_WITHOUT_INTERRUPT,
            per_turn,
            true,
            Acknowledged::TheOneAsked,
            None,
        )
    }

    /// A child that answers an interrupt by naming a request nobody made, and
    /// then fails the turn on its own account.
    pub(crate) fn stream_json_harness_answering_another_request(per_turn: &[&str]) -> HarnessSpec {
        harness_replaying(INIT, per_turn, true, Acknowledged::AnotherRequest, None)
    }

    fn harness_replaying(
        init: &str,
        per_turn: &[&str],
        turn_after_turn: bool,
        acknowledged: Acknowledged,
        heard: Option<&std::path::Path>,
    ) -> HarnessSpec {
        let mut replay = String::new();
        for line in per_turn {
            assert!(
                !line.contains('\''),
                "a recorded protocol line may not carry a single quote: {line}"
            );
            replay.push_str(&format!("printf '%s\\n' '{line}'\n"));
        }
        // The one place the script interpolates rather than quoting a recording:
        // the id it echoes is a value it read at runtime.
        let echoed = match acknowledged {
            Acknowledged::TheOneAsked => "\"$asked\"",
            Acknowledged::AnotherRequest => "nobody-asked-this",
        };
        let mut script = format!("sleep 0.2\nprintf '%s\\n' '{init}'\n");
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
             printf '%s\\n' '{FAILED_RESULT}'\n\
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
                summary: "Read bridge/src/app.rs".to_string()
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::ToolResult {
                summary: "Read: fn main() {}".to_string()
            }
        );
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Narration {
                summary: "dropped the index".to_string()
            }
        );
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
    /// A session that has not announced yet answers no — it has no turn to stop
    /// either.
    #[test]
    fn stopping_a_turn_is_offered_exactly_when_the_child_announced_it() {
        let announced = open(&stream_json_harness(&[THINKING]));
        assert!(
            !announced.can_interrupt(),
            "a child that has said nothing has announced nothing"
        );
        assert!(
            matches!(announced.interrupt(), Err(HarnessError::Unsupported(_))),
            "and the flag and the call answer from the same value"
        );

        wait_for_status(&announced, AgentStatus::Waiting);
        assert!(announced.can_interrupt());
        assert!(announced.interrupt().is_ok());
        announced.end();

        let silent = open(&stream_json_harness_without_interrupt(&[THINKING]));
        wait_for_status(&silent, AgentStatus::Waiting);
        assert!(!silent.can_interrupt());
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

        session.interrupt().expect("this child advertised one");
        session.interrupt().expect("asking twice is allowed");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut written = Vec::new();
        while Instant::now() < deadline && written.len() < 2 {
            written = std::fs::read_to_string(&capture)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("a protocol line"))
                .collect();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(written.len(), 2, "one line per ask: {written:?}");
        for asked in &written {
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
                        AgentActivity::ToolUse { summary } => format!("tool_use: {summary}"),
                        AgentActivity::ToolResult { summary } => format!("tool_result: {summary}"),
                        AgentActivity::Narration { summary } => format!("narration: {summary}"),
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
}
