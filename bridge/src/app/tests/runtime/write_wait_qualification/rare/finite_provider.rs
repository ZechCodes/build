use crate::app::tests::*;
use crate::harness::{
    ActivityReport, OpenedSession, SessionOutput, SessionStatusSnapshot, TurnReceiptSnapshot,
};
use crate::test_support::agent_session::FixtureSessions;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

pub(super) struct FiniteProvider {
    _sessions: FixtureSessions,
    pub(super) log: SessionLog,
    pub(super) opens: Arc<AtomicUsize>,
    status: watch::Sender<SessionStatusSnapshot>,
    receipts: watch::Sender<TurnReceiptSnapshot>,
}

impl FiniteProvider {
    pub(super) fn require_enrolled(&self) {
        assert!(
            self._sessions.is_enrolled(),
            "finite provider factory is absent; refusing provider-dependent qualification dispatch"
        );
    }

    pub(super) fn install(root: &Path) -> Self {
        let log = SessionLog::default();
        let logged = log.clone();
        let opens = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&opens);
        let (status, watched_status) =
            watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let (receipts, watched_receipts) = watch::channel(TurnReceiptSnapshot::default());
        let sessions = FixtureSessions::install(
            std::fs::canonicalize(root).unwrap(),
            Arc::new(move |provider, request| {
                assert_eq!(request.choice.provider, provider);
                counted.fetch_add(1, Ordering::SeqCst);
                let (activity, _) = broadcast::channel::<ActivityReport>(16);
                let output = SessionOutput::reporting(activity.subscribe(), None);
                Ok(OpenedSession {
                    session: Arc::new(
                        DictatedSession::reporting(AgentStatus::Waiting)
                            .reporting_activity(activity)
                            .recording_into(&logged)
                            .watching_status(watched_status.clone())
                            .watching_receipts(watched_receipts.clone())
                            .natively_accepting(request.choice)
                            .interruptible(),
                    ),
                    output,
                })
            }),
        );
        Self {
            _sessions: sessions,
            log,
            opens,
            status,
            receipts,
        }
    }

    pub(super) fn status(&self, status: AgentStatus) {
        self.status.send_modify(|snapshot| {
            *snapshot = snapshot
                .transition(status)
                .expect("valid finite session transition");
        });
    }

    pub(super) fn consumed(&self, operation: &str) {
        self.receipts
            .send_modify(|receipts| receipts.seen_operation_ids.push(operation.to_owned()));
    }
}
