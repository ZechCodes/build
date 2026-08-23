//! The contract the daemon holds with one live agent session.
//!
//! [`AgentSession`] is what Build needs from an agent, and every session
//! implements it: hand it a turn, ask what it is doing, end it. [`TerminalView`]
//! is a capability on top of that — full access to a harness Build can only see
//! the outside of — and [`AgentSession::terminal`] is the one place the question
//! "does this agent have a basement?" is asked. **The terminal is a capability,
//! not a guarantee.**
//!
//! [`HarnessSession`] is the older, wider contract: it requires a byte stream, a
//! resize and an OS pid of every implementation, because a subprocess in a full
//! PTY ([`crate::pty::PtySession`]) has been the only carrier there has ever
//! been. It is on its way to being that implementation's own business — its
//! terminal vocabulary is how the PTY *satisfies* [`AgentSession`], not the
//! daemon's.

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
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError>;

    /// What the agent is doing right now.
    fn status(&self) -> AgentStatus;

    /// End the session and release whatever it holds.
    fn end(&self);

    /// The terminal, if this session has one. `None` is a normal answer.
    ///
    /// An `Option` rather than a second trait object stored beside the session:
    /// it makes the capability a question with one answer, asked in one place,
    /// instead of a flag that can disagree with reality.
    fn terminal(&self) -> Option<&dyn TerminalView> {
        None
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

/// One live agent session.
///
/// The three questions with no transport-free answer are [`ready_within`],
/// [`write_prompt`] and [`idle_for`]: a PTY answers them by watching the paint
/// (a bracketed-paste announcement, a framed paste plus a trailing submit key,
/// the age of the last byte), while a session protocol would answer them from
/// its own turn boundaries. Keeping them behind this trait is what lets a
/// second carrier exist without the daemon learning a second vocabulary.
///
/// [`ready_within`]: HarnessSession::ready_within
/// [`write_prompt`]: HarnessSession::write_prompt
/// [`idle_for`]: HarnessSession::idle_for
pub trait HarnessSession: Send + Sync {
    /// Hand the agent a turn and submit it.
    fn write_prompt(&self, prompt: &str) -> Result<(), HarnessError>;

    /// Forward raw bytes from a human at an attached terminal, untouched.
    fn write_input(&self, bytes: &[u8]) -> Result<(), HarnessError>;

    /// Whether the session will accept a turn within `timeout`. False for a
    /// session that dies first, and false — not a panic — at the deadline.
    fn ready_within(&self, timeout: Duration) -> bool;

    /// How long the session has produced nothing. What "working" is measured
    /// against.
    fn idle_for(&self) -> Duration;

    /// Subscribe to the session's output. Every subscriber sees each chunk from
    /// the moment it subscribes and observes `Closed` once the session ends.
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;

    /// Tell the session the human's viewport changed.
    fn resize(&self, size: PtySize) -> Result<(), HarnessError>;

    /// The OS process id behind the session, while it is still running.
    fn pid(&self) -> Option<u32>;

    /// Whether the session has ended (crash, completion, kill).
    fn has_exited(&self) -> bool;

    /// Whether the session ends within `timeout`. A single [`has_exited`] poll
    /// is racy — a dying harness closes its end before its status is reapable —
    /// so a caller deciding whether a failed write means "crashed" rather than
    /// "wedged" waits the lag out here.
    ///
    /// [`has_exited`]: HarnessSession::has_exited
    fn exited_within(&self, timeout: Duration) -> bool;

    /// The exit code once the session has ended. Stable across repeated calls:
    /// the status is reapable exactly once, so an implementation must cache it.
    fn exit_code(&self) -> Option<i32>;

    /// End the session and release its process-table entry. Killing without
    /// reaping leaks a zombie per session on a daemon that never restarts.
    fn kill_and_reap(&self);

    /// Age the last-output stamp, so a live session reports the silence of one
    /// that has been sitting idle for `ago`.
    ///
    /// Test-only. The windows this feeds are minutes long, and a suite that
    /// waited them out in real time would be unrunnable.
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration);
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
        fn end(&self) {}
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
