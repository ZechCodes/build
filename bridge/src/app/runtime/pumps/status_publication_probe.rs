//! Correctness seam for the actual private observer, including stale-session refusals.
use super::*;
use crate::harness::{SessionStatusSnapshot, TurnContext};
use std::cell::RefCell;

type BeforePublication = Box<dyn FnOnce() + Send>;

thread_local! {
    static BEFORE_PUBLICATION: RefCell<Option<BeforePublication>> = const { RefCell::new(None) };
}

struct HookScope;

impl Drop for HookScope {
    fn drop(&mut self) {
        BEFORE_PUBLICATION.with(|hook| hook.borrow_mut().take());
    }
}

pub(super) fn before_final_publication() {
    let callback = BEFORE_PUBLICATION.with(|hook| hook.borrow_mut().take());
    if let Some(callback) = callback {
        callback();
    }
}

pub(in crate::app) struct StatusPublicationProbe(StatusPump, Option<BeforePublication>);

impl StatusPublicationProbe {
    pub(in crate::app) fn new(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        session: &Arc<dyn AgentSession>,
        instance: SessionInstance,
    ) -> Self {
        let pump = StatusPump {
            state: Arc::downgrade(state),
            key,
            session: Arc::downgrade(session),
            instance,
            frame_clock: Some(Arc::clone(&state.lock().unwrap().frame_clock)),
            recorded_context: None,
            recorded_context_reset_count: 0,
            recorded_limit: super::super::usage_limits::UsageObservation::default(),
        };
        Self(pump, None)
    }

    pub(in crate::app) fn observe(&mut self, snapshot: &SessionStatusSnapshot) -> bool {
        BEFORE_PUBLICATION.with(|hook| {
            assert!(hook.borrow().is_none(), "status test hooks cannot nest");
            *hook.borrow_mut() = self.1.take();
        });
        let _scope = HookScope;
        self.0.observe(snapshot)
    }

    pub(in crate::app) fn before_final_publication(
        &mut self,
        hook: impl FnOnce() + Send + 'static,
    ) {
        assert!(self.1.replace(Box::new(hook)).is_none());
    }

    pub(in crate::app) fn recorded_context(&self) -> (Option<TurnContext>, u64) {
        (self.0.recorded_context, self.0.recorded_context_reset_count)
    }
}
