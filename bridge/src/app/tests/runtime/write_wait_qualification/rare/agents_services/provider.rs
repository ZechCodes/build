use super::*;
use crate::harness::{ActivityReport, OpenedSession, SessionOutput, SessionStatusSnapshot, TurnReceiptSnapshot};
use crate::test_support::agent_session::FixtureSessions;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

pub(super) struct FiniteProvider {
    _sessions: FixtureSessions,
    _activity: broadcast::Sender<ActivityReport>,
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
        let (status, watched_status) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let (receipts, watched_receipts) = watch::channel(TurnReceiptSnapshot::default());
        let (activity, _) = broadcast::channel(16);
        let emitted = activity.clone();
        let sessions = FixtureSessions::install(std::fs::canonicalize(root).unwrap(), Arc::new(move |_, request| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(OpenedSession {
                session: Arc::new(DictatedSession::reporting(AgentStatus::Waiting)
                    .recording_into(&logged)
                    .watching_status(watched_status.clone())
                    .watching_receipts(watched_receipts.clone())
                    .natively_accepting(request.choice)
                    .interruptible()),
                output: SessionOutput::reporting(emitted.subscribe(), None),
            })
        }));
        Self {_sessions:sessions,_activity:activity,log,opens,status,receipts}
    }

    pub(super) fn status(&self, status: AgentStatus) {
        self.status.send_modify(|snapshot| {
            *snapshot = snapshot.transition(status).expect("valid finite session transition");
        });
    }

    pub(super) fn consumed(&self, operation: &str) {
        self.receipts.send_modify(|receipts| receipts.seen_operation_ids.push(operation.to_owned()));
    }
}
