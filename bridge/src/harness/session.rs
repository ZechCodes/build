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
use tokio::sync::broadcast;

/// Things that can go wrong starting or driving a harness session.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("harness session error: {0}")]
    Session(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("harness {binary:?} not found in PATH {path:?} — install it, or restart the daemon from a shell that can see it")]
    NotFound { binary: String, path: String },
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
    /// **May block, and callers must not hold the app-wide state lock across
    /// it.** The PTY implementation writes a framed paste and the harness's
    /// submit key trails it by [`REAL_TUI_SUBMIT_DELAY`], so a call can hold
    /// its thread for seconds; a protocol implementation returns as soon as
    /// the turn is written and never waits on the model. The daemon already
    /// honours this — the verbs queue turns and
    /// `deliver_pending_agent_turns` drains the queue once the lock is free —
    /// and it is written down here so a future caller does not re-learn it
    /// from a deadlock.
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

    /// Age the evidence-of-work stamp, so a live session reports the silence of
    /// one that has been sitting idle for `ago`.
    ///
    /// Test-only, and it travels with [`quiet_for`](AgentSession::quiet_for):
    /// the windows that clock feeds are minutes long, and a suite that waited
    /// them out in real time would be unrunnable.
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration);
}

/// A session's byte stream, subscribed at the moment it opened.
///
/// `None` for a session with no terminal: it paints nothing, so there is no
/// stream to carry and nothing to pump it into.
pub type SessionOutput = Option<broadcast::Receiver<Vec<u8>>>;

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
