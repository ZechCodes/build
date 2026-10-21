use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;

#[cfg(test)]
mod tests;

pub type ResumeWork = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct AdmissionState {
    active: usize,
    closed: bool,
    claiming: Option<u64>,
    next_claim: u64,
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
}

impl Clone for AdmissionLease {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl Drop for LeaseInner {
    fn drop(&mut self) {
        self.gate.state.lock().unwrap().active -= 1;
    }
}

impl AdmissionGate {
    pub(super) fn new(closed: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AdmissionState {
                closed,
                ..AdmissionState::default()
            }),
            ..Self::default()
        })
    }

    pub fn try_enter(self: &Arc<Self>) -> Option<AdmissionLease> {
        self.enter(false)
    }

    /// Reads may finish while an idle probe runs, and their leases prevent
    /// the final handoff from closing admission until they have finished.
    pub fn try_enter_read(self: &Arc<Self>) -> Option<AdmissionLease> {
        self.enter(true)
    }

    fn enter(self: &Arc<Self>, read_only: bool) -> Option<AdmissionLease> {
        let mut state = self.state.lock().unwrap();
        if state.closed || (!read_only && state.claiming.is_some()) {
            return None;
        }
        state.active += 1;
        Some(AdmissionLease(Arc::new(LeaseInner {
            gate: Arc::clone(self),
        })))
    }

    pub async fn enter_when_open(self: &Arc<Self>) -> AdmissionLease {
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

    pub(super) fn claim_idle(&self, busy: impl FnOnce() -> bool) -> bool {
        let claim = {
            let mut state = self.state.lock().unwrap();
            if state.closed || state.claiming.is_some() || state.active != 0 {
                return false;
            }
            state.next_claim = state.next_claim.wrapping_add(1);
            let claim = state.next_claim;
            state.claiming = Some(claim);
            claim
        };
        // A probe can inspect session state or panic. Neither may keep the
        // admission mutex from readers, lease drops, or cancellation.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(busy));
        let closed = self.finish_claim(claim, matches!(&result, Ok(false)));
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
        closed
    }

    fn finish_claim(&self, claim: u64, quiet: bool) -> bool {
        let closed = {
            let mut state = self.state.lock().unwrap();
            if state.claiming != Some(claim) {
                return false;
            }
            state.claiming = None;
            state.closed = quiet && state.active == 0;
            state.closed
        };
        if !closed {
            self.resume();
        }
        closed
    }

    pub(super) fn reopen(&self) {
        let resumed = {
            let mut state = self.state.lock().unwrap();
            let resumed = state.closed || state.claiming.is_some();
            state.closed = false;
            state.claiming = None;
            resumed
        };
        if resumed {
            self.resume();
        }
    }

    fn resume(&self) {
        self.opened.notify_waiters();
        if let Some(resume) = self.resume_work.get() {
            resume();
        }
    }

    pub fn set_resume_work(&self, resume: ResumeWork) -> Result<(), ResumeWork> {
        self.resume_work.set(resume)
    }
}
