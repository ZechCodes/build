//! Ending a session, off the app mutex.
//!
//! [`AgentSession::end`] is kill THEN reap, and SIGKILL does not land on a
//! child wedged in uninterruptible I/O until that I/O returns. It is therefore
//! an unbounded process wait, and every verb that closes a tab used to make it
//! with the app mutex in hand: one stuck harness stopped the daemon.
//!
//! A [`Retirement`] is that wait, moved to a thread of its own. One thread per
//! kill and never a queue — a child that parks its reaper parks nobody else's
//! — and no lock of any kind is held while it runs.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::harness::AgentSession;

/// A session being killed and reaped somewhere else.
///
/// The receipt is what a caller waits on when it has to — a checkout cannot be
/// removed out from under a process still writing into it — and what every
/// other caller drops, because the tab left the registry when the retirement
/// began and that is what makes the agent unaddressable.
#[derive(Clone)]
pub struct Retirement {
    reaped: Arc<(Mutex<bool>, Condvar)>,
}

impl Retirement {
    /// Kill and reap `session` on a thread of its own. Returns before either
    /// has happened.
    pub fn begin(session: Arc<dyn AgentSession>) -> Retirement {
        let reaped = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&reaped);
        // A std thread and not a spawned task: the synchronous unit tests run
        // with no runtime under them, and a tab still has to end there.
        std::thread::spawn(move || {
            session.end();
            let (done, waiting) = &*signal;
            *done.lock().unwrap() = true;
            waiting.notify_all();
        });
        Retirement { reaped }
    }

    /// Whether the process is reaped, waiting up to `timeout` for it.
    ///
    /// Callable only with the app mutex released: it is the wait the daemon's
    /// one rule forbids, made explicit so the callers that genuinely need it
    /// are the ones that say so.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (done, waiting) = &*self.reaped;
        let (done, _) = waiting
            .wait_timeout_while(done.lock().unwrap(), timeout, |done| !*done)
            .expect("a retirement's thread never panics while holding this");
        *done
    }
}
