//! Fixture-only exclusion of explicitly enrolled, already-published notices.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use write_wait::{
    Obligation, WriteCohort, WriteContext, WriteId, WriteSnapshot, WriteWaitRegistry,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum OptionalNoticeKind {
    TrackerWatchNotice,
    CompleteReminder,
}
impl OptionalNoticeKind {
    fn method(self) -> &'static str {
        match self {
            Self::TrackerWatchNotice => "optional.tracker_watch_notice",
            Self::CompleteReminder => "optional.complete_reminder",
        }
    }
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
    /// Actual typed candidate notices, including candidates refused qualification.
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
#[derive(Clone)]
struct Source {
    original_ids: Vec<String>,
    kind: OptionalNoticeKind,
}
#[derive(Default)]
struct NoticeState {
    sources: BTreeMap<WriteId, Source>,
    cancelled_for_cleanup: BTreeSet<WriteId>,
}
pub(crate) struct OptionalNotices {
    registry: Arc<WriteWaitRegistry>,
    enrolled: BTreeSet<(String, OptionalNoticeKind)>,
    state: Mutex<NoticeState>,
    offered: AtomicUsize,
    next: AtomicU64,
}
impl FrameClock {
    pub(crate) fn capture_optional_notices(
        &self,
        enrollments: Vec<(String, OptionalNoticeKind)>,
    ) -> Arc<OptionalNotices> {
        let registry = self
            .write_wait_registry
            .read()
            .unwrap()
            .clone()
            .expect("optional notice enrollment requires this fixture's registry");
        let policy = Arc::new(OptionalNotices {
            registry,
            enrolled: enrollments.into_iter().collect(),
            state: Mutex::new(NoticeState::default()),
            offered: AtomicUsize::new(0),
            next: AtomicU64::new(1),
        });
        *self.optional_notices.write().unwrap() = Some(policy.clone());
        policy
    }
    pub(crate) fn optional_notice_policy(&self) -> Option<Arc<OptionalNotices>> {
        self.optional_notices.read().unwrap().clone()
    }
}
impl OptionalNotices {
    pub(crate) fn offer(&self, context: &WriteContext, kind: OptionalNoticeKind) -> bool {
        if !context.belongs_to(&self.registry) {
            return false;
        }
        self.offered.fetch_add(1, Ordering::Relaxed);
        let ids = context.original_ids();
        !ids.is_empty()
            && ids
                .iter()
                .all(|id| self.enrolled.contains(&(id.clone(), kind)))
    }
    /// Only the app adapter's exact accepted-publication proof allows this call.
    pub(crate) fn begin(&self, context: &WriteContext, kind: OptionalNoticeKind) -> Obligation {
        let original_ids = context.original_ids();
        assert!(context.belongs_to(&self.registry));
        assert!(
            !original_ids.is_empty()
                && original_ids
                    .iter()
                    .all(|id| self.enrolled.contains(&(id.clone(), kind)))
        );
        let id = format!(
            "optional-notice/{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        );
        let root = context.begin_optional_notice(id, kind.method(), self.registry.now());
        let child = root.child();
        self.state
            .lock()
            .unwrap()
            .sources
            .insert(root.id(), Source { original_ids, kind });
        root.admitted();
        root.complete();
        child
    }
    pub(crate) fn samples(&self) -> Vec<OptionalNoticeSample> {
        // Copy mappings first; a concurrently added notice cannot be looked up
        // in an older registry snapshot. Never hold this leaf lock across it.
        let sources = self.state.lock().unwrap().sources.clone();
        let records: BTreeMap<_, _> = self
            .registry
            .snapshot()
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let now = self.registry.now();
        sources
            .into_iter()
            .map(|(id, source)| {
                let delivery = records
                    .get(&id)
                    .expect("excluded notice must remain visible")
                    .clone();
                let elapsed = delivery
                    .completed_at
                    .unwrap_or(now)
                    .duration_since(delivery.intake);
                OptionalNoticeSample {
                    original_ids: source.original_ids,
                    kind: source.kind,
                    delivery,
                    elapsed,
                }
            })
            .collect()
    }
    pub(crate) fn summary(&self) -> OptionalNoticeSummary {
        let samples = self.samples();
        let completed: Vec<_> = samples
            .iter()
            .filter(|s| {
                s.delivery.completed_at.is_some() && !s.delivery.cancelled && !s.delivery.abandoned
            })
            .map(|s| s.elapsed)
            .collect();
        let active: Vec<_> = samples
            .iter()
            .filter(|s| s.delivery.completed_at.is_none())
            .map(|s| s.elapsed)
            .collect();
        let (completed_p95, completed_maximum) = percentiles(&completed);
        let (active_age_p95, active_age_maximum) = percentiles(&active);
        OptionalNoticeSummary {
            offered: self.offered.load(Ordering::Relaxed),
            created: samples.len(),
            completed: completed.len(),
            active: active.len(),
            cancelled_for_cleanup: self.state.lock().unwrap().cancelled_for_cleanup.len(),
            completed_p95,
            completed_maximum,
            active_age_p95,
            active_age_maximum,
        }
    }
    pub(crate) fn cleanup_candidates(&self) -> Result<BTreeSet<WriteId>, &'static str> {
        let active = self.registry.active_snapshot();
        if active
            .iter()
            .any(|row| row.cohort != WriteCohort::OptionalNotice && row.completed_at.is_none())
        {
            return Err("required logical work is still active");
        }
        let state = self.state.lock().unwrap();
        Ok(active
            .iter()
            .filter(|row| {
                row.cohort == WriteCohort::OptionalNotice
                    && row.completed_at.is_none()
                    && state.sources.contains_key(&row.id)
            })
            .map(|row| row.id)
            .collect())
    }
    /// A queue turn may contain several optional roots; any required/untracked
    /// origin keeps the whole turn. A running turn is never inspected here.
    pub(crate) fn removable_origins(
        &self,
        contexts: &[WriteContext],
        candidates: &BTreeSet<WriteId>,
    ) -> Option<BTreeSet<WriteId>> {
        if contexts.is_empty()
            || contexts
                .iter()
                .any(|context| !context.belongs_to(&self.registry))
        {
            return None;
        }
        let ids: BTreeSet<_> = contexts
            .iter()
            .flat_map(|context| context.origins().iter().copied())
            .collect();
        (!ids.is_empty() && ids.is_subset(candidates)).then_some(ids)
    }
    pub(crate) fn record_cleanup(&self, removed: BTreeSet<WriteId>) -> usize {
        let count = removed.len();
        self.state
            .lock()
            .unwrap()
            .cancelled_for_cleanup
            .extend(removed);
        count
    }
}
fn percentiles(values: &[Duration]) -> (Duration, Duration) {
    if values.is_empty() {
        return (Duration::ZERO, Duration::ZERO);
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    (
        ordered[(ordered.len() * 95).div_ceil(100) - 1],
        *ordered.last().unwrap(),
    )
}
