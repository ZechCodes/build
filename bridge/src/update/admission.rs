use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;

pub type ResumeWork = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct AdmissionState {
    active: usize,
    closed: bool,
}

#[derive(Default)]
pub struct AdmissionGate {
    state: Mutex<AdmissionState>,
    opened: Notify,
    resume_work: OnceLock<ResumeWork>,
}

pub struct AdmissionLease(Arc<LeaseInner>);

struct LeaseInner {
    gate: Arc<AdmissionGate>,
    // The final release can run after ACK, so attribution needs a live owner.
    #[cfg(test)]
    cleanup: Option<crate::timing::write_wait::Obligation>,
}

impl Clone for AdmissionLease {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        #[cfg(test)]
        let context = self.cleanup.as_ref().map(|cleanup| cleanup.context());
        {
            #[cfg(test)]
            let _scope = context.as_ref().map(|context| context.bind_sync());
            self.gate.lock_state().active -= 1;
        }
        #[cfg(test)]
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.complete();
        }
    }
}

impl AdmissionGate {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, AdmissionState> {
        #[cfg(test)]
        let waiting = crate::timing::write_wait::current_interval(
            crate::timing::write_wait::IntervalCategory::KnownWait(
                crate::timing::write_wait::WaitKind::Admission,
            ),
        );
        let result = self.state.lock();
        #[cfg(test)]
        drop(waiting);
        result.unwrap()
    }

    pub(super) fn new(closed: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AdmissionState { active: 0, closed }),
            ..Self::default()
        })
    }

    pub fn try_enter(self: &Arc<Self>) -> Option<AdmissionLease> {
        let mut state = self.lock_state();
        if state.closed {
            return None;
        }
        state.active += 1;
        Some(AdmissionLease(Arc::new(LeaseInner {
            gate: Arc::clone(self),
            #[cfg(test)]
            cleanup: crate::timing::write_wait::current_context().map(|context| context.child()),
        })))
    }

    pub async fn enter_when_open(self: &Arc<Self>) -> AdmissionLease {
        #[cfg(test)]
        {
            self.enter_when_open_with_context(crate::timing::write_wait::current_context())
                .await
        }
        #[cfg(not(test))]
        loop {
            let opened = self.opened.notified();
            tokio::pin!(opened);
            opened.as_mut().enable();
            if let Some(lease) = self.try_enter() {
                return lease;
            }
            opened.await;
        }
    }

    // Carry attribution explicitly; the synchronous binding ends before await.
    #[cfg(test)]
    pub(crate) async fn enter_when_open_with_context(
        self: &Arc<Self>,
        context: Option<crate::timing::write_wait::WriteContext>,
    ) -> AdmissionLease {
        loop {
            let opened = self.opened.notified();
            tokio::pin!(opened);
            opened.as_mut().enable();
            let lease = {
                let _scope = context.as_ref().map(|context| context.bind_sync());
                self.try_enter()
            };
            if let Some(lease) = lease {
                return lease;
            }
            let _waiting = context.as_ref().map(|context| {
                context.interval(crate::timing::write_wait::IntervalCategory::KnownWait(
                    crate::timing::write_wait::WaitKind::Admission,
                ))
            });
            opened.await;
        }
    }

    pub(super) fn claim_idle(&self, busy: impl FnOnce() -> bool) -> bool {
        let mut state = self.lock_state();
        if state.closed || state.active != 0 {
            return false;
        }
        state.closed = true;
        if busy() {
            state.closed = false;
            drop(state);
            self.opened.notify_waiters();
            if let Some(resume) = self.resume_work.get() {
                resume();
            }
            return false;
        }
        true
    }

    pub(super) fn reopen(&self) {
        let resumed = {
            let mut state = self.lock_state();
            if !state.closed {
                false
            } else {
                state.closed = false;
                true
            }
        };
        if resumed {
            self.opened.notify_waiters();
            if let Some(resume) = self.resume_work.get() {
                resume();
            }
        }
    }

    pub fn set_resume_work(&self, resume: ResumeWork) -> Result<(), ResumeWork> {
        self.resume_work.set(resume)
    }
}

#[cfg(test)]
#[path = "admission/write_wait_tests.rs"]
mod write_wait_tests;
