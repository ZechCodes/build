//! Drive the actual activity consumer without broadcast scheduling noise.
use super::*;

pub(in crate::app) struct ActivityProbe(Arc<ActivityPump>);

impl ActivityProbe {
    pub(in crate::app) fn new(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        session: &Arc<dyn AgentSession>,
        instance: SessionInstance,
    ) -> Self {
        Self(Arc::new(ActivityPump {
            state: Arc::downgrade(state),
            key,
            session: Arc::downgrade(session),
            instance,
            frame_clock: Arc::clone(&state.lock().unwrap().frame_clock),
        }))
    }

    pub(in crate::app) async fn observed_report(
        &self,
        report: crate::harness::ActivityReport,
    ) -> bool {
        self.0.observe_report(report).await
    }

    pub(in crate::app) fn report_with_timer(
        &self,
        report: &crate::harness::ActivityReport,
        timer: crate::timing::FrameTimer,
    ) -> bool {
        self.0.consume_report(report, timer)
    }

    pub(in crate::app) fn report(&self, report: &crate::harness::ActivityReport) -> bool {
        let timer = self.0.frame_clock.frame("agent.activity");
        self.0.consume_report(report, timer)
    }
}
