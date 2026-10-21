//! Opt-in logical-write qualification, independent of frame completion.
//!
//! This first-stage scaffold preserves the intended API so the semantic tests
//! can fail against missing accounting before implementation. It has no hooks.
use arc_swap::ArcSwap;
use imbl::OrdMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteCohort {
    PendingIntake,
    SemanticWrite,
    ExemptRead,
    ExemptProtocol,
}

pub(crate) const WAIT_BUDGET: Duration = Duration::from_secs(5);
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
type WatchSink = Arc<dyn Fn(WriteSnapshot) + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WriteId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ObligationId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WaitId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitKind {
    Admission,
    Dispatch,
    Permit,
    BlockingPool,
    AppLock,
    StoreWriter,
    ResourceLock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntervalCategory {
    KnownWait(WaitKind),
    SqlService,
    /// Caller-labelled external-service wall time, including nested waits.
    /// This is reported separately and is not exclusive active I/O time.
    OwnExternal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveInterval {
    pub id: WaitId,
    pub obligation: ObligationId,
    pub category: IntervalCategory,
    pub started: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WriteSnapshot {
    pub id: WriteId,
    pub original_id: String,
    pub method: String,
    pub cohort: WriteCohort,
    pub intake: Instant,
    pub admitted_at: Option<Instant>,
    pub acknowledged_at: Option<Instant>,
    pub root_finished: bool,
    pub outstanding: Vec<ObligationId>,
    pub active: Vec<ActiveInterval>,
    pub known_wait_union: Duration,
    pub sql_service_union: Duration,
    pub blocked_or_sql_union: Duration,
    pub conservative_upper: Duration,
    /// External-service wall union; nested waits are not subtracted.
    pub own_external_union: Duration,
    /// Union(external, known wait, SQL) minus union(known wait, SQL).
    pub own_external_active: Duration,
    pub max_chargeable_interval: Duration,
    pub interval_breached: bool,
    pub cumulative_breached: bool,
    pub cancelled: bool,
    pub abandoned: bool,
}

impl WriteSnapshot {
    pub(crate) fn breached(&self) -> bool {
        self.interval_breached || self.cumulative_breached
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompletedInterval {
    pub id: WaitId,
    pub origins: Vec<WriteId>,
    pub obligation: ObligationId,
    pub category: IntervalCategory,
    pub started: Instant,
    pub ended: Instant,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CollectorOverhead {
    pub writer_acquisitions: u64,
    pub writer_lock_wait: Duration,
    pub writer_mutation_hold: Duration,
    pub max_writer_lock_wait: Duration,
    pub max_writer_mutation_hold: Duration,
}
pub(super) struct ActiveWriteCapture(Arc<OrdMap<WriteId, WriteSnapshot>>);
impl ActiveWriteCapture {
    pub(super) fn snapshot_at(&self, _now: Instant) -> Vec<WriteSnapshot> {
        self.0.values().cloned().collect()
    }
}

pub(crate) struct WriteWaitRegistry {
    clock: Clock,
    next: AtomicU64,
    closing: AtomicU64,
    // Only mutators use this lock. The independent watchdog must never do so.
    writer: Mutex<()>,
    // Only live obligations belong here; completed raw/archive rows stay separate.
    active: ArcSwap<OrdMap<WriteId, WriteSnapshot>>,
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
            writer: Mutex::new(()),
            active: ArcSwap::from_pointee(OrdMap::new()),
        })
    }

    fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn begin(
        self: &Arc<Self>,
        original_id: impl Into<String>,
        method: impl Into<String>,
        intake: Instant,
    ) -> LogicalWrite {
        let id = WriteId(self.next());
        let obligation = ObligationId(self.next());
        let row = WriteSnapshot {
            id,
            original_id: original_id.into(),
            method: method.into(),
            cohort: WriteCohort::SemanticWrite,
            intake,
            admitted_at: None,
            acknowledged_at: None,
            root_finished: false,
            outstanding: vec![obligation],
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
        let _writer = self.writer.lock().unwrap();
        let mut active = (**self.active.load()).clone();
        active.insert(id, row);
        self.active.store(Arc::new(active));
        LogicalWrite {
            context: WriteContext {
                registry: Arc::clone(self),
                origins: vec![id],
                obligation,
            },
        }
    }

    pub(crate) fn begin_pending(
        self: &Arc<Self>,
        correlation_id: impl Into<String>,
        intake: Instant,
    ) -> LogicalWrite {
        let write = self.begin(correlation_id, "rpc.unclassified", intake);
        let _writer = self.writer.lock().unwrap();
        let mut active = (**self.active.load()).clone();
        let mut row = active.get(&write.id()).unwrap().clone();
        row.cohort = WriteCohort::PendingIntake;
        active.insert(write.id(), row);
        self.active.store(Arc::new(active));
        write
    }
    pub(super) fn capture_active(&self) -> ActiveWriteCapture {
        ActiveWriteCapture(self.active.load_full())
    }
    pub(crate) fn completed_intervals(&self) -> Vec<CompletedInterval> {
        vec![]
    }
    pub(crate) fn collector_overhead(&self) -> CollectorOverhead {
        CollectorOverhead::default()
    }

    pub(crate) fn coalesced(self: &Arc<Self>, origins: &[WriteId]) -> Obligation {
        assert!(!origins.is_empty(), "an obligation needs an original write");
        Obligation {
            context: WriteContext {
                registry: Arc::clone(self),
                origins: origins.to_vec(),
                obligation: ObligationId(self.next()),
            },
        }
    }

    /// Scaffold: lifecycle, accounting and archival are implemented after RED.
    pub(crate) fn snapshot(&self) -> Vec<WriteSnapshot> {
        self.active_snapshot()
    }

    /// The watchdog reads only this immutable active index, never the writer lock.
    pub(crate) fn active_snapshot(&self) -> Vec<WriteSnapshot> {
        let _now = (self.clock)();
        self.active.load().values().cloned().collect()
    }

    pub(crate) fn watchdog(self: &Arc<Self>, period: Duration, sink: WatchSink) -> Watchdog {
        let registry = Arc::clone(self);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                for row in registry.active_snapshot() {
                    if row.breached() {
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

pub(crate) struct LogicalWrite {
    context: WriteContext,
}

impl LogicalWrite {
    pub(crate) fn id(&self) -> WriteId {
        self.context.origins[0]
    }
    pub(crate) fn context(&self) -> WriteContext {
        self.context.clone()
    }
    pub(crate) fn child(&self) -> Obligation {
        self.context.child()
    }
    pub(crate) fn classify(
        &self,
        _original_id: impl Into<String>,
        _method: impl Into<String>,
        _cohort: WriteCohort,
    ) {
    }
    pub(crate) fn admitted(&self) {}
    pub(crate) fn acknowledged(&self) {}
    pub(crate) fn complete(self) {}
    pub(crate) fn cancel(self) {}
}
impl Drop for LogicalWrite {
    fn drop(&mut self) {}
}

pub(crate) struct Obligation {
    context: WriteContext,
}
impl Obligation {
    pub(crate) fn id(&self) -> ObligationId {
        self.context.obligation
    }
    pub(crate) fn context(&self) -> WriteContext {
        self.context.clone()
    }
    pub(crate) fn child(&self) -> Obligation {
        self.context.child()
    }
    pub(crate) fn interval(&self, category: IntervalCategory) -> WaitGuard {
        self.context.interval(category)
    }
    pub(crate) fn complete(self) {}
    pub(crate) fn cancel(self) {}
}
impl Drop for Obligation {
    fn drop(&mut self) {}
}

/// Cloneable attribution only. Holding this does not complete or create work.
#[derive(Clone)]
pub(crate) struct WriteContext {
    registry: Arc<WriteWaitRegistry>,
    origins: Vec<WriteId>,
    obligation: ObligationId,
}
impl WriteContext {
    pub(crate) fn original_ids(&self) -> Vec<String> {
        vec![]
    }
    pub(crate) fn origins(&self) -> &[WriteId] {
        &self.origins
    }
    pub(crate) fn child(&self) -> Obligation {
        self.registry.coalesced(&self.origins)
    }
    pub(crate) fn interval(&self, category: IntervalCategory) -> WaitGuard {
        WaitGuard {
            id: WaitId(self.registry.next()),
            category,
            registry: Arc::clone(&self.registry),
        }
    }
    /// Synchronous scopes only: callers must carry context explicitly across await.
    pub(crate) fn bind_sync(&self) -> SyncContextGuard {
        let previous = CURRENT.with(|current| current.replace(Some(self.clone())));
        SyncContextGuard {
            previous,
            not_send: std::marker::PhantomData,
        }
    }
}

pub(crate) struct WaitGuard {
    id: WaitId,
    category: IntervalCategory,
    registry: Arc<WriteWaitRegistry>,
}
impl WaitGuard {
    pub(crate) fn id(&self) -> WaitId {
        self.id
    }
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        let _category = self.category;
        self.registry.closing.store(self.id.0, Ordering::Release);
    }
}

thread_local! { static CURRENT: RefCell<Option<WriteContext>> = const { RefCell::new(None) }; }
pub(crate) fn current_context() -> Option<WriteContext> {
    CURRENT.with(|current| current.borrow().clone())
}
pub(crate) fn current_interval(category: IntervalCategory) -> Option<WaitGuard> {
    CURRENT.with(|current| {
        current
            .borrow()
            .as_ref()
            .map(|context| context.interval(category))
    })
}
pub(crate) struct SyncContextGuard {
    previous: Option<WriteContext>,
    not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for SyncContextGuard {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

pub(crate) struct Watchdog {
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            thread.join().unwrap();
        }
    }
}
