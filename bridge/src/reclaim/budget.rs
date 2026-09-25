//! How much work measuring one workspace may do.
//!
//! Every walk the service makes (activity, size, build output, removal) spends
//! from one budget per workspace. The budget has an entry count, a deadline,
//! and the daemon's stop flag. A walk that runs out stops where it is and says
//! so, and whatever it was measuring is treated as not known. One slow tree
//! then costs its own verdict, not every later workspace's, and a daemon going
//! down is not held by a sweep that is still walking.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a walk looks at the clock, in entries.
const CLOCK_EVERY: u64 = 1024;

/// A walk ran out of budget, or the daemon asked it to stop, before it
/// finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unfinished;

/// What one workspace's measurement may still spend.
pub struct Budget {
    deadline: Instant,
    entries: Cell<u64>,
    stop: Arc<AtomicBool>,
}

impl Budget {
    pub fn new(entries: u64, time: Duration, stop: Arc<AtomicBool>) -> Self {
        Self {
            deadline: Instant::now() + time,
            entries: Cell::new(entries),
            stop,
        }
    }

    /// Spend one entry of a walk. `Err` once the entries or the time are gone,
    /// or the daemon is stopping.
    pub fn spend(&self) -> Result<(), Unfinished> {
        let left = self.entries.get();
        if left == 0 || self.stopped() {
            return Err(Unfinished);
        }
        self.entries.set(left - 1);
        if left.is_multiple_of(CLOCK_EVERY) && Instant::now() >= self.deadline {
            self.entries.set(0);
            return Err(Unfinished);
        }
        Ok(())
    }

    /// Whether anything is left to spend, checked before a step no walk can
    /// interrupt (a repository's Git measurement).
    pub fn check(&self) -> Result<(), Unfinished> {
        if self.entries.get() == 0 || self.stopped() || Instant::now() >= self.deadline {
            return Err(Unfinished);
        }
        Ok(())
    }

    /// Whether the daemon asked the service to stop.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}
