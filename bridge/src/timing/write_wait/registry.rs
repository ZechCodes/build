use super::*;
use records::{Record, RegistryState};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(super) enum Finish {
    Complete,
    Cancel,
    Abandon,
}

impl WriteWaitRegistry {
    pub(crate) fn new() -> Arc<Self> {
        Self::with_clock(Arc::new(Instant::now))
    }
    pub(crate) fn with_clock(clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            clock,
            next: AtomicU64::new(1),
            closing: AtomicU64::new(0),
            writer: Mutex::new(RegistryState::default()),
            active: ArcSwap::from_pointee(OrdMap::new()),
            ever_breached: Arc::new(AtomicBool::new(false)),
            overhead: OverheadCounters::default(),
        })
    }
    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    // Misuse is rejected after releasing the guard, so a caught diagnostic
    // panic cannot poison the collector or hide subsequent watchdog evidence.
    fn mutate<T>(&self, change: impl FnOnce(&mut RegistryState) -> Result<T, &'static str>) -> T {
        let started = Instant::now();
        let (result, waited, held) = {
            let mut state = self.writer.lock().unwrap();
            let acquired = Instant::now();
            let result = change(&mut state);
            if result.is_ok() {
                self.active.store(Arc::new(state.active.clone()));
            }
            (result, acquired.duration_since(started), acquired.elapsed())
        };
        self.overhead.record(waited, held);
        result.expect("logical-write instrumentation misuse")
    }
    pub(crate) fn begin(
        self: &Arc<Self>,
        original_id: impl Into<String>,
        method: impl Into<String>,
        intake: Instant,
    ) -> LogicalWrite {
        self.begin_in_cohort(
            original_id.into(),
            method.into(),
            intake,
            WriteCohort::SemanticWrite,
        )
    }
    pub(crate) fn begin_background(
        self: &Arc<Self>,
        original_id: impl Into<String>,
        method: impl Into<String>,
        intake: Instant,
    ) -> LogicalWrite {
        self.begin_in_cohort(
            original_id.into(),
            method.into(),
            intake,
            WriteCohort::BackgroundWork,
        )
    }
    pub(crate) fn now(&self) -> Instant {
        (self.clock)()
    }
    pub(super) fn begin_optional_notice(
        self: &Arc<Self>,
        source: &WriteContext,
        original_id: String,
        method: String,
        intake: Instant,
    ) -> LogicalWrite {
        self.begin_checked(
            original_id,
            method,
            intake,
            WriteCohort::OptionalNotice,
            Some(source),
        )
    }
    pub(crate) fn begin_pending(
        self: &Arc<Self>,
        correlation_id: impl Into<String>,
        intake: Instant,
    ) -> LogicalWrite {
        self.begin_in_cohort(
            correlation_id.into(),
            "rpc.unclassified".into(),
            intake,
            WriteCohort::PendingIntake,
        )
    }
    fn begin_in_cohort(
        self: &Arc<Self>,
        original_id: String,
        method: String,
        intake: Instant,
        cohort: WriteCohort,
    ) -> LogicalWrite {
        self.begin_checked(original_id, method, intake, cohort, None)
    }
    fn begin_checked(
        self: &Arc<Self>,
        original_id: String,
        method: String,
        intake: Instant,
        cohort: WriteCohort,
        source: Option<&WriteContext>,
    ) -> LogicalWrite {
        let id = WriteId(self.next());
        let obligation = ObligationId(self.next());
        let row = WriteSnapshot {
            id,
            original_id,
            method,
            cohort,
            intake,
            admitted_at: None,
            acknowledged_at: None,
            completed_at: None,
            root_finished: false,
            outstanding: vec![],
            active: vec![],
            known_wait_union: Duration::ZERO,
            sql_service_union: Duration::ZERO,
            blocked_or_sql_union: Duration::ZERO,
            conservative_upper: Duration::ZERO,
            own_external_union: Duration::ZERO,
            own_external_active: Duration::ZERO,
            max_chargeable_interval: Duration::ZERO,
            interval_breached: false,
            cumulative_breached: false,
            cancelled: false,
            abandoned: false,
        };
        let record = Record {
            row,
            outstanding: BTreeSet::from([obligation]),
            active: Default::default(),
            closed: Default::default(),
            last_terminal_event: None,
            breaches: records::Breaches::new(Arc::clone(&self.ever_breached)),
        };
        self.mutate(|state| {
            if source.is_some_and(|source| !state.context_live(source)) {
                return Err("optional notice requires a live originating obligation");
            }
            state.active.insert(id, Arc::new(record));
            Ok(())
        });
        LogicalWrite {
            context: WriteContext {
                registry: Arc::clone(self),
                origins: vec![id],
                obligation,
            },
            finished: false,
        }
    }
    pub(crate) fn coalesced(self: &Arc<Self>, origins: &[WriteId]) -> Obligation {
        self.add_child(origins, None)
    }
    pub(super) fn add_child(
        self: &Arc<Self>,
        origins: &[WriteId],
        parent: Option<ObligationId>,
    ) -> Obligation {
        self.add_child_checked(origins, |state| {
            origins.iter().all(|id| {
                state.active.get(id).is_some_and(|record| {
                    parent.map_or(!record.outstanding.is_empty(), |parent| {
                        record.outstanding.contains(&parent)
                    })
                })
            })
        })
    }
    pub(super) fn coalesced_contexts(
        self: &Arc<Self>,
        origins: &[WriteId],
        contexts: &[WriteContext],
    ) -> Obligation {
        self.add_child_checked(origins, |state| {
            contexts.iter().all(|context| state.context_live(context))
        })
    }
    fn add_child_checked(
        self: &Arc<Self>,
        origins: &[WriteId],
        live: impl FnOnce(&RegistryState) -> bool,
    ) -> Obligation {
        let origins: Vec<_> = origins
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let obligation = ObligationId(self.next());
        self.mutate(|state| {
            if origins.is_empty() || !live(state) {
                return Err("child requires live owning obligations");
            }
            for id in &origins {
                let mut record = (**state.active.get(id).unwrap()).clone();
                record.outstanding.insert(obligation);
                state.active.insert(*id, Arc::new(record));
            }
            Ok(())
        });
        Obligation {
            context: WriteContext {
                registry: Arc::clone(self),
                origins,
                obligation,
            },
            finished: false,
        }
    }
    pub(super) fn classify(
        &self,
        context: &WriteContext,
        original_id: String,
        method: String,
        cohort: WriteCohort,
    ) {
        self.mutate(|state| {
            if cohort == WriteCohort::PendingIntake || !state.context_live(context) {
                return Err("classification requires live root and final cohort");
            }
            let id = context.origins[0];
            let mut record = (**state.active.get(&id).unwrap()).clone();
            let row = &mut record.row;
            let identical =
                row.original_id == original_id && row.method == method && row.cohort == cohort;
            if row.cohort != WriteCohort::PendingIntake && !identical {
                return Err("conflicting final classification");
            }
            row.original_id = original_id;
            row.method = method;
            row.cohort = cohort;
            state.active.insert(id, Arc::new(record));
            Ok(())
        });
    }
    pub(super) fn capture_active(&self) -> ActiveWriteCapture {
        ActiveWriteCapture(self.active.load_full())
    }
    pub(super) fn milestone(&self, context: &WriteContext, acknowledged: bool) {
        let now = (self.clock)();
        self.mutate(|state| {
            if !state.context_live(context) {
                return Err("milestone requires live root");
            }
            for id in &context.origins {
                let mut record = (**state.active.get(id).unwrap()).clone();
                let slot = if acknowledged {
                    &mut record.row.acknowledged_at
                } else {
                    &mut record.row.admitted_at
                };
                slot.get_or_insert(now);
                state.active.insert(*id, Arc::new(record));
            }
            Ok(())
        });
    }
    pub(super) fn finish(&self, context: &WriteContext, root: bool, outcome: Finish) {
        let now = (self.clock)();
        self.mutate(|state| {
            if !state.context_live(context) {
                return Err("obligation already finished");
            }
            for id in &context.origins {
                let mut record = (**state.active.get(id).unwrap()).clone();
                record.outstanding.remove(&context.obligation);
                record.observe_terminal_event(now);
                record.row.root_finished |= root;
                record.row.cancelled |= matches!(outcome, Finish::Cancel);
                record.row.abandoned |= matches!(outcome, Finish::Abandon);
                state.update(*id, record, now);
            }
            Ok(())
        });
    }
    pub(super) fn start_interval(
        &self,
        owner: &WriteContext,
        context: &WriteContext,
        category: IntervalCategory,
    ) -> (WaitId, Arc<OnceLock<Instant>>) {
        let id = WaitId(self.next());
        let started = (self.clock)();
        let ended = Arc::new(OnceLock::new());
        self.mutate(|state| {
            if !state.context_live(owner) {
                return Err("interval requires live owning obligation");
            }
            for origin in &context.origins {
                let mut record = (**state.active.get(origin).unwrap()).clone();
                record.active.insert(
                    id,
                    records::OpenInterval {
                        row: ActiveInterval {
                            id,
                            obligation: context.obligation,
                            category,
                            started,
                        },
                        ended: Arc::clone(&ended),
                    },
                );
                state.active.insert(*origin, Arc::new(record));
            }
            Ok(())
        });
        (id, ended)
    }
    pub(super) fn close_interval(&self, context: &WriteContext, id: WaitId) {
        let now = (self.clock)();
        self.mutate(|state| {
            if !context.origins.iter().all(|origin| {
                state
                    .active
                    .get(origin)
                    .is_some_and(|record| record.active.contains_key(&id))
            }) {
                return Err("interval already closed");
            }
            let interval = &state.active.get(&context.origins[0]).unwrap().active[&id];
            state.raw_intervals.push(CompletedInterval {
                id,
                origins: context.origins.clone(),
                obligation: context.obligation,
                category: interval.row.category,
                started: interval.row.started,
                ended: *interval
                    .ended
                    .get()
                    .expect("guard end precedes collector update"),
            });
            for origin in &context.origins {
                let mut record = (**state.active.get(origin).unwrap()).clone();
                let interval = record.active.remove(&id).unwrap();
                let ended = *interval
                    .ended
                    .get()
                    .expect("guard closes end cell before collector update");
                record.closed.insert(&interval.row, ended);
                record.observe_terminal_event(ended);
                state.update(*origin, record, now);
            }
            Ok(())
        });
    }
    /// All raw samples, including completed archive. Not the watchdog path.
    pub(crate) fn snapshot(&self) -> Vec<WriteSnapshot> {
        let (mut rows, active) = {
            let state = self.writer.lock().unwrap();
            (state.archive.clone(), state.active.clone())
        };
        let now = (self.clock)();
        rows.extend(active.values().map(|record| record.snapshot(now)));
        rows.sort_unstable_by_key(|row| row.id);
        rows
    }
    /// Explicit final export; never called by the independent watchdog.
    pub(crate) fn completed_intervals(&self) -> Vec<CompletedInterval> {
        self.writer.lock().unwrap().raw_intervals.clone()
    }
    pub(crate) fn collector_overhead(&self) -> CollectorOverhead {
        self.overhead.snapshot()
    }
    pub(crate) fn active_snapshot_with_time(&self) -> (Instant, Vec<WriteSnapshot>) {
        let active = self.active.load_full();
        let now = (self.clock)();
        (
            now,
            active.values().map(|record| record.snapshot(now)).collect(),
        )
    }
    pub(crate) fn active_snapshot(&self) -> Vec<WriteSnapshot> {
        self.active_snapshot_with_time().1
    }
    pub(crate) fn has_breaches(&self) -> bool {
        self.ever_breached.load(Ordering::Acquire)
    }
    pub(crate) fn watchdog(self: &Arc<Self>, period: Duration, sink: WatchSink) -> Watchdog {
        let registry = Arc::clone(self);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let active = registry.active.load_full();
                let now = (registry.clock)();
                for record in active.values() {
                    let row = record.snapshot(now);
                    if row.breached() && !record.breaches.reported.swap(true, Ordering::AcqRel) {
                        sink(row);
                    }
                }
                std::thread::park_timeout(period);
            }
        });
        Watchdog {
            stopped,
            thread: Some(thread),
        }
    }
}

impl OverheadCounters {
    fn record(&self, wait: Duration, hold: Duration) {
        let wait = u64::try_from(wait.as_nanos()).unwrap_or(u64::MAX);
        let hold = u64::try_from(hold.as_nanos()).unwrap_or(u64::MAX);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.wait.fetch_add(wait, Ordering::Relaxed);
        self.hold.fetch_add(hold, Ordering::Relaxed);
        self.max_wait.fetch_max(wait, Ordering::Relaxed);
        self.max_hold.fetch_max(hold, Ordering::Relaxed);
    }
    fn snapshot(&self) -> CollectorOverhead {
        CollectorOverhead {
            writer_acquisitions: self.calls.load(Ordering::Relaxed),
            writer_lock_wait: Duration::from_nanos(self.wait.load(Ordering::Relaxed)),
            writer_mutation_hold: Duration::from_nanos(self.hold.load(Ordering::Relaxed)),
            max_writer_lock_wait: Duration::from_nanos(self.max_wait.load(Ordering::Relaxed)),
            max_writer_mutation_hold: Duration::from_nanos(self.max_hold.load(Ordering::Relaxed)),
        }
    }
}
