//! Opt-in logical-write qualification, independent of frame completion.
//!
//! Independent active records remain readable while application and collector
//! mutators are blocked. Completed rows are archived outside the active index.
use arc_swap::ArcSwap;
use imbl::OrdMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(test)]
#[path = "write_wait/deep_test_support.rs"]
pub(crate) mod deep_test_support;
mod handles;
mod intervals;
pub(crate) mod mutex;
pub(crate) mod native_retry;
mod records;
mod registry;
pub(crate) mod service;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteCohort {
    PendingIntake,
    SemanticWrite,
    BackgroundWork,
    OptionalNotice,
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
    Retirement,
    Publication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IntervalCategory {
    KnownWait(WaitKind),
    SqlService,
    /// Caller-labelled external-service wall time, including nested waits.
    /// This is reported separately and is not exclusive active I/O time.
    OwnExternal,
    /// Only the actual finite service effect/padding, excluding outer preflight.
    FiniteService,
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
    pub completed_at: Option<Instant>,
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
#[derive(Default)]
struct OverheadCounters {
    calls: AtomicU64,
    wait: AtomicU64,
    hold: AtomicU64,
    max_wait: AtomicU64,
    max_hold: AtomicU64,
}

pub(super) struct ActiveWriteCapture(Arc<OrdMap<WriteId, Arc<records::Record>>>);
impl ActiveWriteCapture {
    pub(super) fn snapshot_at(&self, now: Instant) -> Vec<WriteSnapshot> {
        self.0.values().map(|record| record.snapshot(now)).collect()
    }
}

pub(crate) struct WriteWaitRegistry {
    clock: Clock,
    next: AtomicU64,
    closing: AtomicU64,
    writer: Mutex<records::RegistryState>,
    active: ArcSwap<OrdMap<WriteId, Arc<records::Record>>>,
    ever_breached: Arc<AtomicBool>,
    overhead: OverheadCounters,
}

pub(crate) struct LogicalWrite {
    context: WriteContext,
    finished: bool,
}
pub(crate) struct Obligation {
    context: WriteContext,
    finished: bool,
}
/// Cloneable attribution only. Holding this does not complete or create work.
#[derive(Clone)]
pub(crate) struct WriteContext {
    registry: Arc<WriteWaitRegistry>,
    origins: Vec<WriteId>,
    obligation: ObligationId,
}
pub(crate) struct WaitGuard {
    id: WaitId,
    context: WriteContext,
    ended: Arc<OnceLock<Instant>>,
}
thread_local! { static CURRENT: RefCell<Option<WriteContext>> = const { RefCell::new(None) }; }
pub(crate) fn current_context() -> Option<WriteContext> {
    CURRENT.with(|current| current.borrow().clone())
}
pub(crate) fn current_interval(category: IntervalCategory) -> Option<WaitGuard> {
    current_context().map(|context| context.interval(category))
}
pub(crate) struct SyncContextGuard {
    previous: Option<WriteContext>,
    not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}
pub(crate) struct Watchdog {
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
