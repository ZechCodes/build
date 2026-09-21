//! Turns that wait before they wake their agent (#67).
//!
//! Every turn re-reads the agent's whole context, so what a turn costs is that
//! context times the number of turns. An issue notice is posted on its
//! tracker's thread the moment it lands, and the turn that tells the agent to
//! read it waits here instead of going at once: a burst of changes wakes the
//! agent once.
//!
//! The waiting turn is a catch-up turn — it reads everything unread off the
//! thread when it is sent — so one of them per agent covers every line posted
//! before it goes. A second turn for an agent that already has one waiting is
//! not kept; the earlier of the two waits wins.
//!
//! A turn waits one of two ways: until its settle window ends, or with no end
//! of its own, for the next delivery to its agent (the Complete reminder). Both
//! stop waiting the moment anything else is delivered to that agent: a direct
//! message or an assignment is never delayed, and it takes what is waiting
//! with it.

use std::time::{Duration, Instant};

use super::super::types::PendingAgentTurn;

/// How long an idle tracker's notice waits for the rest of its burst.
pub(in crate::app) const NOTICE_SETTLE_WINDOW: Duration = Duration::from_secs(30);

struct WaitingTurn {
    turn: PendingAgentTurn,
    /// When the settle window ends. `None` waits for the next delivery.
    until: Option<Instant>,
}

impl WaitingTurn {
    fn window_ended(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| until <= now)
    }
}

#[derive(Default)]
pub(super) struct SettlingTurns {
    waiting: Vec<WaitingTurn>,
    /// The window end a timer has already been asked to wake the queue at.
    wake_asked_for: Option<Instant>,
}

impl SettlingTurns {
    /// Wait `turn` until `until`, or for the next delivery when `None`.
    pub(super) fn add(&mut self, turn: PendingAgentTurn, until: Option<Instant>) {
        if let Some(joined) = self
            .waiting
            .iter_mut()
            .find(|waiting| waiting.turn.carries(&turn))
        {
            joined.until = earlier(joined.until, until);
            return;
        }
        self.waiting.push(WaitingTurn { turn, until });
    }

    /// Move into `ready` every waiting turn that may go: `can_go` allows it,
    /// and either its window has ended or another turn to its agent is going
    /// now. A ready turn that reads the thread already carries it, so the
    /// waiting one is dropped rather than sent as a second turn.
    pub(super) fn release(
        &mut self,
        ready: &mut Vec<PendingAgentTurn>,
        mut can_go: impl FnMut(&PendingAgentTurn) -> bool,
        now: Instant,
    ) {
        let (going, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|waiting| {
                let goes_with_a_delivery = ready
                    .iter()
                    .any(|turn| turn.tab_key() == waiting.turn.tab_key());
                (goes_with_a_delivery || waiting.window_ended(now)) && can_go(&waiting.turn)
            });
        self.waiting = waiting;
        for released in going {
            if !ready.iter().any(|turn| turn.carries(&released.turn)) {
                ready.push(released.turn);
            }
        }
    }

    /// The window end a timer should wake the queue at, once per end: `None`
    /// when no window is still open or a wake no later than it is already
    /// asked for.
    pub(super) fn wake_due(&mut self, now: Instant) -> Option<Instant> {
        let next = self
            .waiting
            .iter()
            .filter_map(|waiting| waiting.until)
            .filter(|until| *until > now)
            .min()?;
        if self
            .wake_asked_for
            .is_some_and(|asked| asked > now && asked <= next)
        {
            return None;
        }
        self.wake_asked_for = Some(next);
        Some(next)
    }

    pub(super) fn wake_fired(&mut self, at: Instant) {
        if self.wake_asked_for == Some(at) {
            self.wake_asked_for = None;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.waiting.len()
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Drop what a refused request queued since `earlier_len`, the way the
    /// queue does. A turn that joined one already waiting cannot be taken back
    /// out of it; its line stays unread on the thread either way.
    pub(super) fn refuse_since(&mut self, earlier_len: usize) {
        let earlier_len = earlier_len.min(self.waiting.len());
        let mut appended = self.waiting.split_off(earlier_len);
        appended.retain(|waiting| waiting.turn.survives_refusal);
        self.waiting.append(&mut appended);
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&PendingAgentTurn) -> bool) {
        self.waiting.retain(|waiting| keep(&waiting.turn));
    }

    /// End every open window now, for a test that cannot wait thirty seconds.
    #[cfg(test)]
    pub(super) fn lapse(&mut self, now: Instant) {
        for waiting in &mut self.waiting {
            if waiting.until.is_some() {
                waiting.until = Some(now);
            }
        }
    }
}

/// The sooner of two waits, where `None` is no end at all.
fn earlier(one: Option<Instant>, other: Option<Instant>) -> Option<Instant> {
    match (one, other) {
        (Some(one), Some(other)) => Some(one.min(other)),
        (one, other) => one.or(other),
    }
}
