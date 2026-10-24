//! Drive the actual activity consumer without broadcast scheduling noise.
use super::*;

pub(in crate::app) struct ActivityProbe(ActivityPump);

impl ActivityProbe {
    pub(in crate::app) fn new(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        session: &Arc<dyn AgentSession>,
        instance: SessionInstance,
    ) -> Self {
        Self(ActivityPump {
            state: Arc::downgrade(state),
            key,
            session: Arc::downgrade(session),
            instance,
        })
    }

    pub(in crate::app) fn report(&self, report: &crate::harness::ActivityReport) -> bool {
        self.0.reported(report)
    }
}
