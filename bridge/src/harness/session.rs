//! The contract the daemon holds with one live agent session.
//!
//! [`AgentSession`] is what Build needs from an agent, and every session
//! implements it: hand it a turn, ask what it is doing, end it. [`TerminalView`]
//! is a capability on top of that — full access to a harness Build can only see
//! the outside of — and [`AgentSession::terminal`] is the one place the question
//! "does this agent have a basement?" is asked. **The terminal is a capability,
//! not a guarantee.**
//!
//! Together they are the daemon's whole vocabulary for a running agent. The
//! terminal mechanics a full PTY ([`crate::pty::PtySession`]) answers in —
//! paint-settled readiness, a framed paste plus a trailing submit key, the age
//! of the last byte — are that implementation's own business: they are how the
//! PTY *satisfies* [`AgentSession`], and no caller above it names them.

use std::time::Duration;

use portable_pty::PtySize;
use tokio::sync::{broadcast, watch};

use crate::harness::surfaces::AgentSurfaces;

/// Things that can go wrong starting or driving a harness session.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("harness setup error: {0}")]
    Setup(String),
    #[error("harness session error: {0}")]
    Session(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("harness {binary:?} not found in PATH {path:?} — install it, or restart the daemon from a shell that can see it")]
    NotFound { binary: String, path: String },
    /// This carrier cannot do the thing that was asked, and the sentence says
    /// where the thing actually lives. Never a fallback to something different
    /// — the `require_shell_kind` manner: refuse loudly, point at the real
    /// door.
    #[error("{0}")]
    Unsupported(String),
}

/// One turn handed to an agent.
///
/// A value, not keystrokes. A PTY can only take text, so text is all this
/// carries; a session protocol that accepts structured content (attachments,
/// tool results) grows this struct rather than every caller that builds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// What the agent is being asked. Whether it is framed as a paste, spoken
    /// over a protocol or written to a pipe is the session's business.
    pub text: String,
}

impl Turn {
    /// The turn saying `text`.
    pub fn new(text: impl Into<String>) -> Turn {
        Turn { text: text.into() }
    }
}

/// What an agent is doing right now.
///
/// A PTY synthesizes this from the age of its last paint; a session protocol
/// reports it from its own turn boundaries. The distinction the enum exists to
/// preserve is [`Working`](AgentStatus::Working) versus
/// [`Waiting`](AgentStatus::Waiting): a terminal can only guess at it, and a
/// harness that knows must be allowed to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Starting, and not yet able to take a turn.
    Starting,
    /// Mid-turn. The rail dot pulses.
    Working,
    /// Idle at a prompt, waiting for the human.
    Waiting,
    /// Over. The code is `None` for a session with no process behind it.
    Ended { code: Option<i32> },
}

/// What Build needs from an agent. Every session implements this.
pub trait AgentSession: Send + Sync {
    /// Hand the agent one turn. Returns when the turn is accepted, not when it
    /// is finished.
    ///
    /// **Must return promptly: an implementation writes the turn out and
    /// returns, never sleeping out a delay and never waiting on the model.**
    /// Build's main delivery path (`deliver`) takes the session handle out of
    /// the tab registry and hands the turn over with the app-wide state lock
    /// released, but the in-place nudge (`nudge_live_agent_tab`) speaks to a
    /// live tab from under it — so a carrier that blocks here stalls every RPC,
    /// every terminal pump and the idle sweep along with it.
    ///
    /// Both carriers can hold that honestly. The PTY implementation writes the
    /// framed paste and returns, leaving the harness's submit key to be written
    /// off-thread [`REAL_TUI_SUBMIT_DELAY`] later; a protocol implementation
    /// returns as soon as the turn is written to the child's stdin. It is
    /// written down here so an implementer does not learn it from a daemon that
    /// has gone quiet.
    ///
    /// [`REAL_TUI_SUBMIT_DELAY`]: crate::harness::REAL_TUI_SUBMIT_DELAY
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError>;

    /// What the agent is doing right now.
    fn status(&self) -> AgentStatus;

    /// How long since the session last showed evidence of work — bytes painted
    /// for a PTY, protocol events read for a session protocol.
    ///
    /// Not [`status`](AgentSession::status) restated: that is a 30-second
    /// judgement about whether to pulse a rail dot, while this is the
    /// minutes-scale anomaly clock the idle sweep demotes on. Both carriers
    /// can hold one honestly.
    fn quiet_for(&self) -> Duration;

    /// Whether the session ends within `timeout`. A single status poll is
    /// racy: a dying harness closes its pipes BEFORE its exit status is
    /// reapable, so a caller deciding whether a failed [`send_turn`] means
    /// "crashed" rather than "wedged" waits the lag out here.
    ///
    /// [`send_turn`]: AgentSession::send_turn
    fn exited_within(&self, timeout: Duration) -> bool;

    /// End the session and release whatever it holds.
    ///
    /// For a subprocess carrier that includes the process-table entry: killing
    /// without reaping leaks a zombie per session on a daemon that never
    /// restarts.
    fn end(&self);

    /// The session's last words, once it has ended. `None` for a session that
    /// left none worth repeating.
    ///
    /// A PTY answers `None`: its last words are on the screen, which belongs to
    /// the tab and is read there. A session protocol has no screen and answers
    /// with the last error it was *told* — the final result's error text, or
    /// the last line of stderr. Reported, never scraped.
    fn epitaph(&self) -> Option<String> {
        None
    }

    /// The terminal, if this session has one. `None` is a normal answer.
    ///
    /// An `Option` rather than a second trait object stored beside the session:
    /// it makes the capability a question with one answer, asked in one place,
    /// instead of a flag that can disagree with reality.
    fn terminal(&self) -> Option<&dyn TerminalView> {
        None
    }

    /// The session's own account of what it is doing, if it keeps one. `None`
    /// is a normal answer.
    ///
    /// The mirror of [`terminal`](AgentSession::terminal), and the two are
    /// alternatives rather than extras: a CLI wrapper is opaque and offers the
    /// escape hatch, a session protocol reports its reasoning and tool calls
    /// and offers this. A session that answers `None` to BOTH is refused at
    /// the spawn — Build would have no way to see it working and, worse, no
    /// stream whose close performs the death rites, so its tab would read as
    /// live until the idle sweep explained the exit as silence.
    ///
    /// Every subscriber sees each event from the moment it subscribes and
    /// observes `Closed` once the session's stream ends.
    fn activity(&self) -> Option<broadcast::Receiver<ActivityReport>> {
        None
    }

    fn surfaces(&self) -> Option<AgentSurfaces> {
        None
    }

    fn surfaces_changed(&self) -> Option<watch::Receiver<u64>> {
        None
    }

    /// Whether this session can be told to stop the turn it is running.
    ///
    /// Asked without performing it: the agent digest answers the SPA with this
    /// before anyone presses anything. An implementation must answer it from
    /// the same value [`interrupt`](AgentSession::interrupt) refuses on, so the
    /// two cannot disagree — a control the client offers and the session then
    /// refuses is worse than no control at all.
    ///
    /// The rule runs ONE way, and the second half of the question is why: this
    /// asks whether the carrier can stop a turn AND whether there is a turn to
    /// stop. A refusal always means `false`; a `false` may instead mean there
    /// was nothing running, which [`interrupt`](AgentSession::interrupt)
    /// answers with the satisfied no-op rather than a refusal. So a session
    /// reported working with no turn of its own open — background work
    /// outliving the turn that started it — is `working: true` with
    /// `can_interrupt: false`, which the composer renders as the plain Send.
    ///
    /// A defaulted method rather than [`terminal`](AgentSession::terminal)'s
    /// `Option` capability, because this one is announced at RUNTIME — a
    /// protocol carrier learns it from the child's own `init` line, so the same
    /// provider answers differently on two versions of the same CLI.
    fn can_interrupt(&self) -> bool {
        false
    }

    /// Stop the turn the agent is running now, and return.
    ///
    /// **Never kills.** [`end`](AgentSession::end) is the kill and it is a
    /// different verb with a different lifetime: a session that answered
    /// `interrupt` is the SAME session afterwards, still holding its
    /// conversation, ready for the turn Build hands it next.
    ///
    /// Returns promptly, for the reason [`send_turn`](AgentSession::send_turn)
    /// does: the in-place nudge speaks from under the app-wide state lock.
    ///
    /// The default is the terminal carrier's answer, and it is a refusal. ESC
    /// is a keystroke whose meaning belongs to the harness rather than to
    /// Build, a terminal reports no turn boundary — so Build could write the
    /// bytes and never learn whether anything stopped — and the basement is
    /// always accessible, so the human who wants a full harness stopped drops
    /// in and presses Esc with the screen in front of them.
    fn interrupt(&self) -> Result<(), HarnessError> {
        Err(HarnessError::Unsupported(
            "this agent has no interrupt — open its terminal and press Esc".to_string(),
        ))
    }

    /// The id the harness gave the conversation this session is having, once it
    /// has announced one. What a respawn resumes BY NAME.
    ///
    /// `None` for a carrier that names no conversation, and for one that has
    /// not announced yet. Reported, never scraped: it is read off the protocol
    /// line the child sent, never out of a transcript directory.
    fn session_id(&self) -> Option<String> {
        None
    }

    fn active_model(&self) -> Option<String> {
        None
    }

    /// Age the evidence-of-work stamp, so a live session reports the silence of
    /// one that has been sitting idle for `ago`.
    ///
    /// Test-only, and it travels with [`quiet_for`](AgentSession::quiet_for):
    /// the windows that clock feeds are minutes long, and a suite that waited
    /// them out in real time would be unrunnable.
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration);
}

/// One thing an agent reported doing, on its way to the conversation.
///
/// The five kinds are `ThreadEventKind`'s five activity kinds and nothing else:
/// a session that reports its own work has no second tab, no second scrollback
/// and no second input path — its reasoning, tool calls, narration and
/// background work are conversation, classed `Status`, so none of them pulls
/// the human in.
///
/// Each carries the summary the timeline shows. What it costs to build one is
/// the reporting session's business: a protocol carrier renders a tool call as
/// its name plus a one-line input, and never mints the call a human would then
/// read twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentActivity {
    /// The agent thought out loud.
    Reasoning { summary: String },
    /// The agent called a tool. `call_id` is the protocol's own id for the
    /// call — the name its answer will arrive under.
    ToolUse { call_id: String, summary: String },
    /// A tool answered: the completion signal for the [`ToolUse`] carrying the
    /// same id, rather than an event of its own. `summary` is the one-line
    /// answer text, which is empty when the tool said nothing and when no
    /// answer ever came.
    ///
    /// [`ToolUse`]: AgentActivity::ToolUse
    ToolResult {
        call_id: String,
        outcome: ToolOutcome,
        summary: String,
    },
    /// The agent narrated. Distinct from a `post_thread_message`, which is the
    /// agent deliberately addressing the human.
    Narration { summary: String },
    /// Background work the harness runs beyond the turn moved — started,
    /// finished, failed, or said something worth reading.
    TaskUpdate { summary: String },
}

/// How a tool call ended, as the harness saw it.
///
/// `Unanswered` is what a boundary reports rather than what a tool did: the
/// turn ended, or the session did, over a call whose answer never came. It is
/// terminal like the other two — a call that closed this way is not still
/// running and never will be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    Ok,
    Error,
    Unanswered,
}

impl AgentActivity {
    /// The line the timeline shows for this event.
    pub fn summary(&self) -> &str {
        match self {
            AgentActivity::Reasoning { summary }
            | AgentActivity::ToolUse { summary, .. }
            | AgentActivity::ToolResult { summary, .. }
            | AgentActivity::Narration { summary }
            | AgentActivity::TaskUpdate { summary } => summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityReport {
    pub activity: AgentActivity,
    pub parent_call_id: Option<String>,
}

impl ActivityReport {
    pub fn own_work(activity: AgentActivity) -> ActivityReport {
        ActivityReport {
            activity,
            parent_call_id: None,
        }
    }
}

/// What a session says about itself, subscribed at the moment it opened.
///
/// One stream per capability and never both: an opaque CLI wrapper paints
/// bytes, and a session protocol reports activity. Subscribed at the open
/// rather than at the pump because a harness's first words — its startup paint,
/// or a protocol child's `init` — can arrive before the pump is ever spawned.
pub struct SessionOutput {
    /// The terminal's bytes, for a session that offers one.
    pub bytes: Option<broadcast::Receiver<Vec<u8>>>,
    /// The session's own account of its work, for one that keeps it.
    pub activity: Option<broadcast::Receiver<ActivityReport>>,
}

impl SessionOutput {
    /// The output of a session that paints.
    pub fn painting(bytes: broadcast::Receiver<Vec<u8>>) -> SessionOutput {
        SessionOutput {
            bytes: Some(bytes),
            activity: None,
        }
    }

    /// The output of a session that reports what it is doing.
    pub fn reporting(activity: broadcast::Receiver<ActivityReport>) -> SessionOutput {
        SessionOutput {
            bytes: None,
            activity: Some(activity),
        }
    }

    /// Neither stream — what a session Build cannot watch at all would hand
    /// back. [`open_session`](crate::harness::open_session) refuses one.
    pub fn silent() -> SessionOutput {
        SessionOutput {
            bytes: None,
            activity: None,
        }
    }
}

/// Full access to a harness Build can only see the outside of.
///
/// A CLI wrapper is opaque: Build knows what it launched and what it reported,
/// and everything in between is paint. The terminal is the escape hatch for
/// exactly that — the human drops in and sees what Build cannot. A harness that
/// reports its own reasoning and tool calls is not opaque, so it has nothing to
/// escape to, and offers none of this.
pub trait TerminalView: Send + Sync {
    /// Subscribe to the session's output. Every subscriber sees each chunk from
    /// the moment it subscribes and observes `Closed` once the session ends.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;

    /// Forward raw bytes from a human at an attached terminal, untouched.
    fn write_input(&self, bytes: &[u8]) -> Result<(), HarnessError>;

    /// Tell the session the human's viewport changed.
    fn resize(&self, size: PtySize) -> Result<(), HarnessError>;

    /// The OS process id behind the session, while it is still running.
    fn pid(&self) -> Option<u32>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session that reports its own turn boundaries and has nothing to escape
    /// to — the shape [`AgentSession`] exists for.
    struct ProtocolSession;

    impl AgentSession for ProtocolSession {
        fn send_turn(&self, _turn: &Turn) -> Result<(), HarnessError> {
            Ok(())
        }
        fn status(&self) -> AgentStatus {
            AgentStatus::Working
        }
        fn quiet_for(&self) -> Duration {
            Duration::ZERO
        }
        fn exited_within(&self, _timeout: Duration) -> bool {
            false
        }
        fn end(&self) {}
        fn backdate_last_output(&self, _ago: Duration) {}
    }

    /// Both capabilities default to absent, so each carrier declares only the
    /// one it has: an opaque CLI wrapper offers the terminal, a session that
    /// reports its own reasoning and tool calls offers the activity stream.
    #[test]
    fn a_session_that_says_nothing_about_its_capabilities_offers_neither() {
        let session: Box<dyn AgentSession> = Box::new(ProtocolSession);
        assert!(session.terminal().is_none());
        assert!(session.activity().is_none());
    }

    #[test]
    fn a_session_that_says_nothing_about_surfaces_has_none() {
        let session: Box<dyn AgentSession> = Box::new(ProtocolSession);
        assert!(session.surfaces().is_none());
        assert!(session.surfaces_changed().is_none());
    }

    /// Stopping a turn defaults to "no", and the refusal says where the thing
    /// actually lives instead of falling back to something different — the
    /// `require_shell_kind` manner. Naming the conversation defaults to "no"
    /// the same way.
    #[test]
    fn a_session_that_says_nothing_about_stopping_a_turn_refuses_and_says_where_to_go() {
        let session: Box<dyn AgentSession> = Box::new(ProtocolSession);
        assert!(!session.can_interrupt());
        assert!(session.session_id().is_none());

        let refused = session.interrupt().expect_err("the default is a refusal");
        assert!(
            matches!(&refused, HarnessError::Unsupported(said) if said.contains("terminal")
                && said.contains("Esc")),
            "the refusal carries the sentence a bare `None` could not: {refused}"
        );
    }

    /// A refusal implies the flag was false, so the SPA can never offer a
    /// control the session then refuses.
    ///
    /// One way only: a false flag does NOT imply a refusal, because it also
    /// covers a session with no turn to stop — which the call answers with the
    /// satisfied no-op.
    #[test]
    fn a_session_that_refuses_an_interrupt_never_offered_one() {
        let session: Box<dyn AgentSession> = Box::new(ProtocolSession);
        assert!(session.interrupt().is_err());
        assert!(!session.can_interrupt());
    }

    /// The capability defaults to absent, so a harness that is not opaque gets
    /// a terminal-free session without writing a line about terminals.
    #[test]
    fn a_session_that_says_nothing_about_terminals_has_none() {
        let session: Box<dyn AgentSession> = Box::new(ProtocolSession);
        assert!(session.terminal().is_none());
        assert_eq!(session.status(), AgentStatus::Working);
        session.send_turn(&Turn::new("take this turn")).unwrap();
    }
}
