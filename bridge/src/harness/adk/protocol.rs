use crate::harness::surfaces::SurfaceLedger;
use crate::harness::{AgentStatus, SessionStatusSnapshot, TurnChoiceSupport};
use crate::models::{AgentProvider, ModelChoice};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use tokio::sync::watch;

pub(super) const BUILD_MCP_TOOL_PREFIX: &str = "mcp__build__";

/// How much of a tool call's input, or of a tool's answer, one summary carries.
///
/// A tool call is machinery, not something the agent said: a `Write` input is an
/// entire file and a `Read` result can be thousands of lines, and neither
/// belongs in a conversation row whole. Reasoning and narration are NOT capped
/// here — those are the agent's own words, and the conversation carries what an
/// agent says whole.
pub(crate) const TOOL_SUMMARY_LIMIT: usize = 240;

/// How large the activity backlog may grow before a slow subscriber loses the
/// oldest events. Matches the byte pump's window: a turn that calls forty tools
/// while nothing is draining is a reader problem, not a reason to stall the
/// child.
pub(super) const ACTIVITY_BACKLOG: usize = 1024;

/// The capability a child announces in its `init` line when the turn it is
/// running can be stopped over the wire.
///
/// Asked of the child rather than of a version number, because the same CLI
/// answers differently on two releases — and a Build that guessed from the
/// version would offer a control the session then refused.
const INTERRUPT_CAPABILITY: &str = "interrupt_receipt_v1";

/// The interrupt Build asked for, until the result that closes the turn it
/// ended arrives.
pub(super) struct PendingInterrupt {
    pub(super) request_id: String,
    /// The child answered `control_response` for this id — which it does before
    /// it emits the result, so an interrupt still unacked when the result lands
    /// is one the child never acted on.
    pub(super) acked: bool,
    /// A turn was handed over behind the interrupt — the steering turn, which
    /// the child runs once the interrupted one is closed.
    pub(super) steered: bool,
}

/// Everything the protocol has told this session so far.
///
/// Every field is reported rather than inferred, which is the whole difference
/// between this session protocol and a terminal: a PTY guesses `Working` from the age of
/// the last byte, and this one counts turns against the results that closed
/// them.
pub(super) struct ProtocolState {
    /// Whether the child has announced itself (`system` / `init`). Until it
    /// does, the session is starting and cannot take a turn.
    pub(super) announced: bool,
    /// Whether a turn Build handed over is still unanswered.
    ///
    /// A flag rather than a count of turns against results, and the protocol is
    /// why: a message written while the child is mid-turn is absorbed into the
    /// running turn, which still ends in ONE result line. Counting would leave
    /// such a session reporting `Working` for the rest of its life — and since
    /// `Working` short-circuits the idle sweep, an agent that quietly stopped
    /// would never be explained.
    pub(super) turn_open: bool,
    /// When the child last said anything at all — the quiet clock's instant.
    pub(super) last_line: Instant,
    /// The id the child gave this conversation, for `--resume`.
    pub(super) session_id: Option<String>,
    pub(super) model: Option<String>,
    /// What Build asked this child to run — the spawn's `--model`, moved on
    /// by every `set_model` written since. What the `init` line is checked
    /// against, so a child running something else is ended rather than
    /// trusted, the way the codex carrier checks its opened thread.
    pub(super) requested_model: Option<String>,
    /// The spawn's `--effort`. There is no control request that moves it, so
    /// a turn choosing another effort needs a fresh child.
    pub(super) requested_effort: Option<String>,
    /// The `set_model` requests still unanswered, by request id, against the
    /// model each asked for. Answered by the reader: a success moves
    /// [`model`](ProtocolState::model), an error is the session's last words.
    pub(super) pending_model_changes: HashMap<String, String>,
    /// When the first turn was written. The child announces itself only
    /// once it has read a turn (verified against 2.1.236 and 2.1.2xx: no
    /// `init` before the first user line), so the startup deadline counts
    /// from here rather than from the fork.
    pub(super) first_turn_at: Option<Instant>,
    /// Set once Build has ended the session, so the startup watchdog stops
    /// looking at a child that is already being reaped.
    pub(super) closed: bool,
    /// What the child announced it can do, verbatim from its `init` line.
    pub(super) capabilities: Vec<String>,
    /// The interrupt Build is waiting on, if any. At most one: asking twice to
    /// stop the same turn is one ask.
    pub(super) pending_interrupt: Option<PendingInterrupt>,
    /// The last error the child REPORTED, from a result line that carried one.
    pub(super) reported_error: Option<String>,
    /// The last thing the child said on stderr, for a death with no result.
    pub(super) last_stderr_line: Option<String>,
    /// The background work the child says is live right now — its task id
    /// against the description it goes by.
    ///
    /// Reconciled against the child's own roster rather than bookkept from the
    /// start and end events, so it cannot drift from what the harness says is
    /// running. Sorted rather than hashed because the rows it mints are read by
    /// a human in the order they are minted, and a hash order would shuffle two
    /// tasks ending together from run to run.
    pub(super) tasks: BTreeMap<String, String>,
    pub(super) surfaces: SurfaceLedger,
}

impl ProtocolState {
    pub(super) fn new(choice: &ModelChoice) -> ProtocolState {
        ProtocolState {
            announced: false,
            turn_open: false,
            last_line: Instant::now(),
            session_id: None,
            model: None,
            requested_model: choice.model.clone(),
            requested_effort: choice.effort.clone(),
            pending_model_changes: HashMap::new(),
            first_turn_at: None,
            closed: false,
            capabilities: Vec::new(),
            pending_interrupt: None,
            reported_error: None,
            last_stderr_line: None,
            tasks: BTreeMap::new(),
            surfaces: SurfaceLedger::default(),
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
    pub(super) fn live_status(&self) -> AgentStatus {
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
    pub(super) fn announces_interrupt(&self) -> bool {
        self.capabilities
            .iter()
            .any(|announced| announced == INTERRUPT_CAPABILITY)
    }

    /// Whether a frozen choice can be applied to this child in place.
    ///
    /// The codex rule, in claude's shape: the provider has to be this one,
    /// the model can move (`set_model` is a control request the child
    /// takes on the same pipe) but cannot be CLEARED back to the CLI's
    /// default once one was named, and the effort cannot move at all — the
    /// CLI takes it on argv and answers no control request for it (probed
    /// against `update_settings`: refused for a session source).
    pub(super) fn turn_choice_support(&self, choice: &ModelChoice) -> TurnChoiceSupport {
        let native_provider = choice.provider == AgentProvider::ClaudeAdk;
        let model_supported = choice.model.is_some() || self.requested_model.is_none();
        let effort_supported = choice.effort == self.requested_effort;
        if native_provider && model_supported && effort_supported {
            TurnChoiceSupport::Native
        } else {
            TurnChoiceSupport::RestartRequired
        }
    }
}

pub(super) fn publish_status(updates: &watch::Sender<SessionStatusSnapshot>, status: AgentStatus) {
    updates.send_if_modified(|snapshot| {
        let Some(next) = snapshot.transition(status) else {
            return false;
        };
        *snapshot = next;
        true
    });
}

/// What became of one tool call, kept until its result arrives so the answer
/// can be paired to it — or so it can be closed as unanswered when the turn
/// ends first. A call that was Build's own is remembered too, so that its answer
/// stays as silent as the call was.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RecordedCall {
    Minted {
        tool: String,
        parent_call_id: Option<String>,
    },
    BuildsOwn,
}

pub(super) const SURFACE_TASK_SUBTYPES: [&str; 4] = [
    "task_started",
    "task_progress",
    "task_updated",
    "task_notification",
];
