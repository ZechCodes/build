//! Opt-in arrival pulse for real frame lock acquisitions in fault controls.
use super::*;

pub(super) type LockWaitObserver = dyn Fn(&str, Instant) + Send + Sync;

impl FrameClock {
    pub(crate) fn observe_lock_waits(&self, observer: Arc<LockWaitObserver>) {
        *self.lock_wait_observer.write().unwrap() = Some(observer);
    }

    pub(super) fn mark_lock_wait(&self, method: &str, at: Instant) {
        let observer = self.lock_wait_observer.read().unwrap().clone();
        if let Some(observer) = observer {
            observer(method, at);
        }
    }
}
