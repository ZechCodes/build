//! Fixture-only optional cohort; real paths default to Required until proven otherwise.
use super::*;
use write_wait::WriteSnapshot;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OptionalNoticeKind {
    TrackerWatchNotice,
    CompleteReminder,
}

#[derive(Clone, Debug)]
pub(crate) struct OptionalNoticeSample {
    pub original_ids: Vec<String>,
    pub kind: OptionalNoticeKind,
    pub delivery: WriteSnapshot,
    /// Completed lifetime, or explicitly censored current age when still active.
    pub elapsed: Duration,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OptionalNoticeSummary {
    pub offered: usize,
    pub created: usize,
    pub completed: usize,
    pub active: usize,
    pub cancelled_for_cleanup: usize,
    pub completed_p95: Duration,
    pub completed_maximum: Duration,
    pub active_age_p95: Duration,
    pub active_age_maximum: Duration,
}

pub(crate) struct OptionalNotices;
impl FrameClock {
    pub(crate) fn capture_optional_notices(
        &self,
        _enrollments: Vec<(String, OptionalNoticeKind)>,
    ) -> Arc<OptionalNotices> {
        let policy = Arc::new(OptionalNotices);
        *self.optional_notices.write().unwrap() = Some(policy.clone());
        policy
    }
}
impl OptionalNotices {
    pub(crate) fn samples(&self) -> Vec<OptionalNoticeSample> {
        Vec::new()
    }
    pub(crate) fn summary(&self) -> OptionalNoticeSummary {
        OptionalNoticeSummary::default()
    }
}
