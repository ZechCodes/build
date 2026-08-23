//! The contract the daemon holds with one live agent session.
//!
//! Build talks to an agent through exactly the calls on [`HarnessSession`]:
//! give it a turn, tell me when it can take one, tell me whether it is working,
//! give me its output, and let me end it. Nothing above this trait knows how
//! those are carried. Today the only carrier is a subprocess in a full PTY
//! ([`crate::pty::PtySession`]), which is why the vocabulary is a terminal's —
//! a resize, a byte stream, an OS pid. A carrier that is not a terminal answers
//! the same questions from its own protocol instead of from painted bytes.

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
