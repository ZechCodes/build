use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Breaches {
    interval: AtomicBool,
    cumulative: AtomicBool,
    pub reported: AtomicBool,
    global: Arc<AtomicBool>,
}
impl Breaches {
    pub(super) fn new(global: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            interval: AtomicBool::new(false),
            cumulative: AtomicBool::new(false),
            reported: AtomicBool::new(false),
            global,
        })
    }
    fn observe(&self, totals: &intervals::Totals) {
        let interval = totals.longest > WAIT_BUDGET;
        let cumulative = totals.known + totals.sql > WAIT_BUDGET;
        if interval {
            self.interval.store(true, Ordering::Release);
        }
        if cumulative {
            self.cumulative.store(true, Ordering::Release);
        }
        if interval || cumulative {
            self.global.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone)]
pub(super) struct OpenInterval {
    pub row: ActiveInterval,
    pub ended: Arc<OnceLock<Instant>>,
}

#[derive(Clone)]
pub(super) struct Record {
    pub row: WriteSnapshot,
    pub outstanding: BTreeSet<ObligationId>,
    pub active: BTreeMap<WaitId, OpenInterval>,
    pub closed: intervals::ClosedIntervals,
    pub breaches: Arc<Breaches>,
}
impl Record {
    pub(super) fn snapshot(&self, now: Instant) -> WriteSnapshot {
        let mut row = self.row.clone();
        row.outstanding = self.outstanding.iter().copied().collect();
        let observed: Vec<_> = self
            .active
            .values()
            .map(|interval| (interval.row.clone(), interval.ended.get().copied()))
            .collect();
        row.active = observed
            .iter()
            .filter(|(_, ended)| ended.map_or(true, |end| end > now))
            .map(|(interval, _)| interval.clone())
            .collect();
        let totals = self.closed.measure(&observed, now);
        self.breaches.observe(&totals);
        row.known_wait_union = totals.known;
        row.sql_service_union = totals.sql;
        row.blocked_or_sql_union = totals.blocked;
        row.conservative_upper = totals.known + totals.sql;
        row.own_external_union = totals.external;
        row.own_external_active = totals.external_active;
        row.max_chargeable_interval = totals.longest;
        row.interval_breached = self.breaches.interval.load(Ordering::Acquire);
        row.cumulative_breached = self.breaches.cumulative.load(Ordering::Acquire);
        row
    }
    pub(super) fn finished(&self) -> bool {
        self.outstanding.is_empty() && self.active.is_empty()
    }
}

#[derive(Default)]
pub(super) struct RegistryState {
    pub active: OrdMap<WriteId, Arc<Record>>,
    pub archive: Vec<WriteSnapshot>,
    pub raw_intervals: Vec<CompletedInterval>,
}
impl RegistryState {
    pub(super) fn update(&mut self, id: WriteId, record: Record, now: Instant) {
        let row = record.snapshot(now);
        if record.finished() {
            self.active.remove(&id);
            self.archive.push(row);
        } else {
            self.active.insert(id, Arc::new(record));
        }
    }
    pub(super) fn context_live(&self, context: &WriteContext) -> bool {
        context.origins.iter().all(|id| {
            self.active
                .get(id)
                .is_some_and(|record| record.outstanding.contains(&context.obligation))
        })
    }
}
