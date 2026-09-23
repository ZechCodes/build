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
            state: Mutex::new(AdmissionState { active: 0, closed }),
            ..Self::default()
        })
    }

    pub fn try_enter(self: &Arc<Self>) -> Option<AdmissionLease> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
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
        let mut state = self.state.lock().unwrap();
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
            let mut state = self.state.lock().unwrap();
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
