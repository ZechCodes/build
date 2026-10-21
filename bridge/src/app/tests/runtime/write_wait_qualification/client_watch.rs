//! Independent client deadlines, including unsent scheduled originals.
//!
//! Register the complete immutable schedule before waking any Tokio caller.
//! Call `offer` at the scheduler's actual release, before acquiring a caller's
//! two writer credits. `sent`, `server_registered`, `admitted`, and `complete`
//! record distinct observed boundaries; this module invents no server rows.
//!
//! The std watchdog owns its deadline index. It never locks Registry, Evidence,
//! AppState, a collector, or a writer. Its clock must also be independent of
//! those locks (use Arc::new(Instant::now) in actual trials). Archival snapshots
//! may take caller-evidence locks and are deliberately outside its poll path.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
pub(super) type DecodedObserver = Arc<dyn Fn(&str, Instant) + Send + Sync>;
const LIMIT: Duration = Duration::from_secs(5);
// A memory safety bound, not a workload credit or a mechanism to drop intents.
// Exceeding it fails setup explicitly. The driver can specify its exact bound.
const DEFAULT_MAX_ORIGINALS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum TrafficCohort {
    SemanticWrite,
    Read,
    Exempt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FinalOutcome {
    Reply,
    ProtocolError,
    TransportError,
}

#[derive(Clone, Debug)]
pub(super) struct ClientRow {
    pub original_id: String,
    pub cohort: TrafficCohort,
    pub intended_at: Instant,
    pub registered_at: Instant,
    /// Compatibility-only enrollment through offer(). Qualification rejects it.
    pub implicit_schedule: bool,
    pub offered_at: Option<Instant>,
    pub sent_at: Option<Instant>,
    pub server_registered_at: Option<Instant>,
    pub admitted_at: Option<Instant>,
    /// The observed final encoded reply/transport terminal, never a cancellation.
    pub final_at: Option<Instant>,
    pub final_outcome: Option<FinalOutcome>,
    pub cancelled_at: Option<Instant>,
    /// Settlement timestamp retained for the original four-control API.
    pub finished_at: Option<Instant>,
    pub cancelled: bool,
    pub breached: bool,
}

struct Evidence {
    row: Mutex<Lifecycle>,
    deadline: Arc<Deadline>,
}

/// Actual mutable observations only. No placeholder absolute due time is stored
/// for a relative schedule before its single common start has been published.
struct Lifecycle {
    registered_at: Instant,
    implicit_schedule: bool,
    offered_at: Option<Instant>,
    sent_at: Option<Instant>,
    server_registered_at: Option<Instant>,
    admitted_at: Option<Instant>,
    final_at: Option<Instant>,
    final_outcome: Option<FinalOutcome>,
    cancelled_at: Option<Instant>,
    finished_at: Option<Instant>,
}

#[derive(Clone)]
enum Schedule {
    Absolute(Instant),
    Relative {
        offset: Duration,
        base: Arc<OnceLock<Instant>>,
    },
}

impl Schedule {
    fn intended_at(&self) -> Option<Instant> {
        match self {
            Self::Absolute(at) => Some(*at),
            Self::Relative { offset, base } => base.get().map(|base| {
                base.checked_add(*offset)
                    .expect("validated schedule instant")
            }),
        }
    }

    fn deadline_key(&self) -> DeadlineKey {
        match self {
            Self::Absolute(at) => {
                DeadlineKey::Absolute(at.checked_add(LIMIT).expect("deadline is representable"))
            }
            Self::Relative { offset, .. } => DeadlineKey::Relative(
                offset
                    .checked_add(LIMIT)
                    .expect("relative deadline is representable"),
            ),
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum DeadlineKey {
    Absolute(Instant),
    Relative(Duration),
}

/// Immutable identity/schedule plus independently readable terminal evidence.
/// settled_ns is zero while open; otherwise it is elapsed nanoseconds + 1
/// from Inner.epoch. One atomic publication records the terminal timestamp.
struct Deadline {
    original_id: String,
    cohort: TrafficCohort,
    schedule: Schedule,
    settled_ns: AtomicU64,
    breached: AtomicBool,
    notified: AtomicBool,
}

enum Event {
    Registered(Arc<Deadline>),
    Settled(Arc<Deadline>),
}

struct Registry {
    rows: BTreeMap<String, Arc<Evidence>>,
    receiver: Option<mpsc::Receiver<Event>>,
    latest_registration: Instant,
    maximum_relative_offset: Duration,
}

struct Inner {
    clock: Clock,
    epoch: Instant,
    maximum_originals: usize,
    registry: Mutex<Registry>,
    sender: mpsc::Sender<Event>,
    // Retain a thread handle for unparking; no caller mutex is read by watchdog.
    watchdog_thread: Mutex<Option<thread::Thread>>,
    relative_base: Arc<OnceLock<Instant>>,
    ready: AtomicBool,
    unmatched_replies: Mutex<Vec<String>>,
    decoded_observer: OnceLock<DecodedObserver>,
}

#[derive(Clone)]
pub(super) struct ClientWatch {
    inner: Arc<Inner>,
}

pub(super) struct ClientSnapshot {
    pub sampled_at: Instant,
    pub rows: Vec<ClientRow>,
}

pub(super) struct ScheduledIdentity {
    pub cohort: TrafficCohort,
    /// None means the immutable relative schedule is registered but unreleased.
    pub intended_at: Option<Instant>,
}

pub(super) struct FinalReconciliation {
    pub sampled_at: Instant,
    pub rows: Vec<ClientRow>,
    pub violations: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Breach {
    pub original_id: String,
    pub cohort: TrafficCohort,
    pub intended_at: Instant,
    pub observed_at: Instant,
    pub settled_at: Option<Instant>,
}

struct WatchState {
    stop: AtomicBool,
    failed: AtomicBool,
    polls: AtomicU64,
    inspected_deadlines: AtomicU64,
    indexed: AtomicU64,
}

/// Read-only access to the existing watchdog's atomic status. It owns no
/// receiver, archive lock, deadline index, callback, or additional watchdog.
pub(super) struct ClientWatchObservation {
    state: Arc<WatchState>,
}

pub(super) struct ClientWatchGuard {
    state: Arc<WatchState>,
    handle: Option<JoinHandle<()>>,
    breaches: mpsc::Receiver<Breach>,
}

#[derive(Debug)]
pub(super) struct WatchReport {
    pub failed: bool,
    pub panicked: bool,
    pub polls: u64,
    pub inspected_deadlines: u64,
    pub still_indexed: u64,
    pub breaches: Vec<Breach>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CohortCounts {
    pub elapsed: Duration,
    pub archived: usize,
    pub intended: usize,
    pub registered: usize,
    pub offered: usize,
    pub sent: usize,
    pub server_registered: usize,
    pub admitted: usize,
    pub final_replies: usize,
    pub protocol_errors: usize,
    pub transport_errors: usize,
    pub cancelled: usize,
    pub unfinished: usize,
    pub breached: usize,
    pub implicit_schedules: usize,
}

mod lifecycle;
mod observations;
#[cfg(test)]
mod tests;
mod watchdog;
