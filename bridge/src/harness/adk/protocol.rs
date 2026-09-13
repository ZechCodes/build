use crate::harness::surfaces::SurfaceLedger;
use crate::harness::{AgentStatus, SessionStatusSnapshot};
use std::collections::BTreeMap;
use std::time::Instant;
use tokio::sync::watch;

pub(super) const BUILD_MCP_TOOL_PREFIX: &str = "mcp__build__";

/// How much text a compact machinery preview carries.
///
/// A tool call is machinery, not something the agent said: a `Write` input is an
/// entire file and a `Read` result can be thousands of lines, and neither
/// belongs in a compact status surface whole. Expandable conversation rows use
/// [`ACTIVITY_TEXT_LIMIT`] instead.
pub(crate) const TOOL_SUMMARY_LIMIT: usize = 240;

/// Maximum text stored in one expandable conversation activity event.
/// Whitespace is preserved so opening a row reveals the provider's full shape.
pub(crate) const ACTIVITY_TEXT_LIMIT: usize = 5_000;

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
    pub(super) fn new() -> ProtocolState {
        ProtocolState {
            announced: false,
            turn_open: false,
            last_line: Instant::now(),
            session_id: None,
            model: None,
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
