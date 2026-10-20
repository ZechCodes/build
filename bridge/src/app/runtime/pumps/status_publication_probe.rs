//! Correctness seam for the actual private observer, including stale-session refusals.
use super::*;
use crate::harness::{SessionStatusSnapshot, TurnContext};

pub(in crate::app) struct StatusPublicationProbe(StatusPump);

impl StatusPublicationProbe {
    pub(in crate::app) fn new(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        session: &Arc<dyn AgentSession>,
        instance: SessionInstance,
    ) -> Self {
        Self(StatusPump {
            state: Arc::downgrade(state),
            key,
            session: Arc::downgrade(session),
            instance,
            frame_clock: Some(Arc::clone(&state.lock().unwrap().frame_clock)),
            recorded_context: None,
            recorded_context_reset_count: 0,
            recorded_limit: super::super::usage_limits::UsageObservation::default(),
        })
    }

    pub(in crate::app) fn observe(&mut self, snapshot: &SessionStatusSnapshot) -> bool {
        self.0.observe(snapshot)
    }

    pub(in crate::app) fn recorded_context(&self) -> (Option<TurnContext>, u64) {
        (self.0.recorded_context, self.0.recorded_context_reset_count)
    }
}
