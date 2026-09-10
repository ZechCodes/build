//! Reconnect/retry backoff: exponential doubling with a cap and a healthy-session
//! reset.
//!
//! Extracted as a pure, unit-testable unit because the two places that back off —
//! the relay reconnect loop in `main` and the `done` control-socket accept loop —
//! must get the doubling, the cap, and the reset exactly right. An inline policy
//! (a bare `backoff = (backoff * 2).min(cap)`) can't be tested, so a wrong cap or
//! an inverted reset would ship undetected.

use std::time::Duration;

/// Exponential backoff between a minimum and a maximum delay. `current` starts at
/// `min`, doubles on each [`increase`](Backoff::increase) up to `max`, and returns
/// to `min` on [`reset`](Backoff::reset).
#[derive(Debug, Clone)]
pub struct Backoff {
    current: Duration,
    min: Duration,
    max: Duration,
}

impl Backoff {
    /// A fresh backoff sitting at `min`. `max` is the cap doubling never exceeds.
    pub fn new(min: Duration, max: Duration) -> Self {
        Backoff {
            current: min,
            min,
            max,
        }
    }

    /// The delay to wait before the next attempt.
    pub fn current(&self) -> Duration {
        self.current
    }

    /// Double the delay for the next attempt, capped at `max`.
    pub fn increase(&mut self) {
        self.current = (self.current * 2).min(self.max);
    }

    /// Return to the minimum delay — call after an attempt that succeeded.
    pub fn reset(&mut self) {
        self.current = self.min;
    }

    /// Reset the delay if a just-finished session lasted long enough to count as
    /// healthy (at least `max`): a connection that stayed up should make the next
    /// retry start cheap, while a session that died fast keeps escalating.
    pub fn note_session(&mut self, session_duration: Duration) {
        if session_duration >= self.max {
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn starts_at_the_minimum() {
        let b = Backoff::new(secs(2), secs(30));
        assert_eq!(b.current(), secs(2));
    }

    #[test]
    fn increase_doubles_up_to_the_cap() {
        let mut b = Backoff::new(secs(2), secs(30));
        let mut seen = vec![b.current()];
        for _ in 0..6 {
            b.increase();
            seen.push(b.current());
        }
        // 2 → 4 → 8 → 16 → 30 (cap, not 32) → 30 → 30.
        assert_eq!(
            seen,
            vec![
                secs(2),
                secs(4),
                secs(8),
                secs(16),
                secs(30),
                secs(30),
                secs(30)
            ]
        );
    }

    #[test]
    fn reset_returns_to_the_minimum() {
        let mut b = Backoff::new(secs(2), secs(30));
        b.increase();
        b.increase();
        assert_eq!(b.current(), secs(8));
        b.reset();
        assert_eq!(b.current(), secs(2));
    }

    #[test]
    fn a_healthy_session_resets_but_a_short_one_does_not() {
        let mut b = Backoff::new(secs(2), secs(30));
        b.increase();
        b.increase();
        assert_eq!(b.current(), secs(8));

        // A session shorter than the cap keeps the escalated delay.
        b.note_session(secs(29));
        assert_eq!(b.current(), secs(8), "a short session must not reset");

        // A session that reached the cap counts as healthy → cheap next retry.
        b.note_session(secs(30));
        assert_eq!(b.current(), secs(2), "a healthy session resets the delay");
    }
}
