//! OFF-SOURCE bridge from actual fixture producer closures to trial_complete.
//! No timer/counter is counted as production work. Callbacks MUST execute the
//! real producer body and return errors for its actual verification failures.
use super::trial_complete::{ProducerHandle, ProducerSet, TrialName};
use crate::timing::write_wait::{WriteWaitRegistry, WAIT_BUDGET};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[path = "producer_adapter/tests.rs"]
mod tests;

pub type ActualBody = Box<dyn FnMut(u64) -> Result<(), String> + Send + 'static>;
pub struct ProductionBodies {
    pub status_observations: ActualBody,
    pub delivery_consumption: ActualBody,
    pub source_git_refresh: ActualBody,
    pub watcher_publication: ActualBody,
}
const NO_STOP: u64 = u64::MAX;
const NO_ACTIVE: u64 = u64::MAX;
const WINDOW: Duration = Duration::from_secs(600);

#[derive(Clone, Copy)]
struct Config {
    name: &'static str,
    original_segment: &'static str,
    method: &'static str,
    period: Duration,
}
const CONFIG: [Config; 4] = [
    Config {
        name: "status_observations",
        original_segment: "status",
        method: "agent.poll_status",
        period: Duration::from_millis(250),
    },
    Config {
        name: "delivery_consumption",
        original_segment: "delivery",
        method: "tasks.delivery_service",
        period: Duration::from_millis(250),
    },
    Config {
        name: "source_git_refresh",
        original_segment: "source",
        method: "source.sync",
        period: Duration::from_secs(1),
    },
    Config {
        name: "watcher_publication",
        original_segment: "watcher",
        method: "watcher.publish",
        period: Duration::from_secs(1),
    },
];

#[derive(Clone, Debug)]
pub struct ProducerSnapshot {
    pub name: &'static str,
    pub period_us: u64,
    pub planned: u64,
    pub intended: u64,
    pub started: u64,
    // These mean actual callback outcomes, not delivered messages or observed
    // state changes. Domain effect counts come from real service/session logs.
    pub completed: u64,
    pub failed: u64,
    pub pending_not_started: u64,
    pub missed_at_stop: u64,
    pub active_ordinal: Option<u64>,
    pub maximum_start_lag_us: u64,
    pub maximum_return_lag_us: u64,
    pub oldest_owed_ordinal: Option<u64>,
    pub oldest_owed_age_us: Option<u64>,
    pub deadline_breach: Option<String>,
    pub stop_requested: bool,
    pub body_released: bool,
    pub joined: bool,
    pub first_error: Option<String>,
    pub counters_coherent: bool,
}
struct State {
    config: Config,
    window: Duration,
    start: OnceLock<Instant>,
    stop_elapsed_ns: AtomicU64,
    stop_requested: AtomicBool,
    started: AtomicU64,
    counters_epoch: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    active: AtomicU64,
    maximum_start_lag_ns: AtomicU64,
    maximum_return_lag_ns: AtomicU64,
    next_unsettled: AtomicU64,
    body_released: AtomicBool,
    joined: AtomicBool,
    first_error: OnceLock<String>,
    worker: OnceLock<std::thread::Thread>,
}
enum Command {
    Start(Instant),
    StopBeforeStart,
}
pub struct ActualProducer {
    state: Arc<State>,
    start_command: std::sync::mpsc::Sender<Command>,
    start_sent: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
fn ns(value: Duration) -> u64 {
    value.as_nanos().min(u64::MAX as u128) as u64
}
fn bounded_error(message: String) -> String {
    message.chars().take(4096).collect()
}

impl State {
    fn due(&self, ordinal: u64) -> Option<Instant> {
        self.start
            .get()
            .map(|start| *start + Duration::from_nanos(ns(self.config.period) * ordinal))
    }
    fn oldest_owed(&self, now: Instant) -> Option<(u64, Duration)> {
        let ordinal = self.next_unsettled.load(Ordering::Acquire);
        if ordinal >= self.intended(now)
            || (self.stop_requested.load(Ordering::Acquire)
                && self.active.load(Ordering::Acquire) != ordinal)
        {
            return None;
        }
        Some((ordinal, now.saturating_duration_since(self.due(ordinal)?)))
    }
    fn deadline_error(&self, ordinal: u64, elapsed: Duration) -> Option<String> {
        (elapsed > WAIT_BUDGET).then(|| {
            format!(
                "{} ordinal{ordinal}: original due deadline exceeded: {}us > {}us",
                self.config.name,
                elapsed.as_micros(),
                WAIT_BUDGET.as_micros()
            )
        })
    }
    fn await_due(&self, ordinal: u64, due: Instant) -> Option<Instant> {
        loop {
            if self.stop_requested.load(Ordering::Acquire) {
                return None;
            }
            let now = Instant::now();
            if now < due {
                std::thread::park_timeout(due - now);
                continue;
            }
            if let Some(error) = self.deadline_error(ordinal, now.duration_since(due)) {
                self.fail(error);
                self.stop(now);
                return None;
            }
            return Some(now);
        }
    }
    fn returned(&self, ordinal: u64, due: Instant, returned: Instant) {
        let elapsed = returned.saturating_duration_since(due);
        self.maximum_return_lag_ns
            .fetch_max(ns(elapsed), Ordering::Release);
        self.next_unsettled.store(ordinal + 1, Ordering::Release);
        if let Some(error) = self.deadline_error(ordinal, elapsed) {
            // The actual body outcome remains completed/failed independently.
            // A late return is a schedule violation, never a fresh five seconds.
            self.fail(error);
            self.stop(returned);
        }
    }
    // Independent snapshots read only atomics/immutable OnceLocks. Never app,
    // registry writer, collector, callback/body or join-handle mutexes.
    fn intended(&self, now: Instant) -> u64 {
        let Some(start) = self.start.get() else {
            return 0;
        };
        if now < *start {
            return 0;
        }
        let period = ns(self.config.period);
        let window = ns(self.window);
        let stopped = self.stop_elapsed_ns.load(Ordering::Acquire);
        let elapsed = ns(now.saturating_duration_since(*start));
        // Stop is exclusive: a callback due exactly at stop is not offered.
        if stopped != NO_STOP && stopped <= elapsed {
            let horizon = stopped.min(window);
            return if horizon == 0 {
                0
            } else {
                (horizon - 1) / period + 1
            };
        }
        if elapsed >= window {
            (window - 1) / period + 1
        } else {
            elapsed / period + 1
        }
    }
    fn stop(&self, at: Instant) {
        let offset = self
            .start
            .get()
            .map_or(0, |start| ns(at.saturating_duration_since(*start)));
        self.stop_elapsed_ns.fetch_min(offset, Ordering::AcqRel);
        self.stop_requested.store(true, Ordering::Release);
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
    }
    fn fail(&self, error: String) {
        let _ = self.first_error.set(bounded_error(error));
    }
}

impl ActualProducer {
    fn new(
        name: TrialName,
        config: Config,
        window: Duration,
        registry: Arc<WriteWaitRegistry>,
        runtime: tokio::runtime::Handle,
        mut body: ActualBody,
    ) -> Result<Arc<Self>, String> {
        let state = Arc::new(State {
            config,
            window,
            start: OnceLock::new(),
            stop_elapsed_ns: AtomicU64::new(NO_STOP),
            stop_requested: AtomicBool::new(false),
            started: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            active: AtomicU64::new(NO_ACTIVE),
            counters_epoch: AtomicU64::new(0),
            maximum_start_lag_ns: AtomicU64::new(0),
            maximum_return_lag_ns: AtomicU64::new(0),
            next_unsettled: AtomicU64::new(0),
            body_released: AtomicBool::new(false),
            joined: AtomicBool::new(false),
            first_error: OnceLock::new(),
            worker: OnceLock::new(),
        });
        let worker_state = Arc::clone(&state);
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name(format!("qual-{}", config.original_segment))
            .spawn(move || {
                let _entered_runtime = runtime.enter();
                let _ = worker_state.worker.set(std::thread::current());
                let start = match receive.recv() {
                    Ok(Command::Start(start)) => start,
                    Ok(Command::StopBeforeStart) | Err(_) => {
                        drop(body);
                        worker_state.body_released.store(true, Ordering::Release);
                        return;
                    }
                };
                let finish = start + worker_state.window;
                let planned = (ns(worker_state.window) - 1) / ns(config.period) + 1;
                for ordinal in 0..planned {
                    let due = start
                        + Duration::from_nanos(
                            ns(config.period)
                                .checked_mul(ordinal)
                                .expect("fixed600s ordinal fits u64 nanoseconds"),
                        );
                    // The finite plan itself excludes due >= finish. A body
                    // crossing finish does not cancel an already-offered slot.
                    let Some(actual_start) = worker_state.await_due(ordinal, due) else {
                        break;
                    };
                    worker_state.maximum_start_lag_ns.fetch_max(
                        ns(actual_start.saturating_duration_since(due)),
                        Ordering::Relaxed,
                    );
                    // Root is created BEFORE any fixture body or AppState lock.
                    // This is an actual BackgroundWork root; no fake wait interval
                    // is inserted to replace missing production hook coverage.
                    let original = format!(
                        "qual/{}/background/{}/{ordinal:05}",
                        name.label(),
                        config.original_segment
                    );
                    let root = registry.begin_background(original, config.method, due);
                    let context = root.context();
                    worker_state.counters_epoch.fetch_add(1, Ordering::AcqRel);
                    worker_state.active.store(ordinal, Ordering::Release);
                    worker_state.started.fetch_add(1, Ordering::Release);
                    worker_state.counters_epoch.fetch_add(1, Ordering::Release);
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let binding = context.bind_sync();
                        let result = body(ordinal);
                        drop(binding);
                        result
                    }));
                    let returned = Instant::now();
                    match outcome {
                        Ok(Ok(())) => {
                            root.complete();
                            worker_state.counters_epoch.fetch_add(1, Ordering::AcqRel);
                            worker_state.completed.fetch_add(1, Ordering::Release);
                        }
                        Ok(Err(error)) => {
                            // The real body returned with an observed failure. Its
                            // lifecycle root settles, but the producer/trial fails.
                            root.complete();
                            worker_state.counters_epoch.fetch_add(1, Ordering::AcqRel);
                            worker_state.failed.fetch_add(1, Ordering::Release);
                            worker_state.fail(format!("{} ordinal{ordinal}: {error}", config.name));
                            worker_state.stop(Instant::now());
                        }
                        Err(payload) => {
                            // Dropping a panicked root retains abandonment in the
                            // registry. It is never converted into completion.
                            drop(root);
                            worker_state.counters_epoch.fetch_add(1, Ordering::AcqRel);
                            worker_state.failed.fetch_add(1, Ordering::Release);
                            let error = payload
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| {
                                    payload.downcast_ref::<&str>().map(|s| (*s).to_string())
                                })
                                .unwrap_or_else(|| "non-string panic".into());
                            worker_state.fail(format!(
                                "{} ordinal{ordinal} panicked: {error}",
                                config.name
                            ));
                            worker_state.stop(Instant::now());
                        }
                    }
                    worker_state.returned(ordinal, due, returned);
                    worker_state.active.store(NO_ACTIVE, Ordering::Release);
                    worker_state.counters_epoch.fetch_add(1, Ordering::Release);
                }
                // No Shift/Delay reset: immutable due times and all missed-at-stop
                // slots remain visible. A late body may cause catch-up execution;
                // its real start lag is retained rather than relabelling offered rate.
                worker_state.stop(Instant::now().min(finish));
                drop(body); // releases captured Arc<AppState> before joined=true
                worker_state.body_released.store(true, Ordering::Release);
            })
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(Self {
            state,
            start_command: send,
            start_sent: AtomicBool::new(false),
            thread: Mutex::new(Some(worker)),
        }))
    }
    pub fn snapshot(&self) -> ProducerSnapshot {
        self.snapshot_at(Instant::now())
    }
    fn snapshot_at(&self, now: Instant) -> ProducerSnapshot {
        let state = &self.state;
        // Observe stop before counters: a completed producer's release must
        // not be paired with an older started count and invent missed slots.
        let stopped = state.stop_requested.load(Ordering::Acquire);
        // Bounded retries: the independent watchdog never waits for a stopped
        // or preempted producer writer. Final joined snapshots are coherent.
        let mut counters_coherent = false;
        let mut observed = (
            state.started.load(Ordering::Acquire),
            state.completed.load(Ordering::Acquire),
            state.failed.load(Ordering::Acquire),
            state.active.load(Ordering::Acquire),
        );
        for _ in 0..4 {
            let before = state.counters_epoch.load(Ordering::Acquire);
            if before % 2 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let counts = (
                state.started.load(Ordering::Acquire),
                state.completed.load(Ordering::Acquire),
                state.failed.load(Ordering::Acquire),
                state.active.load(Ordering::Acquire),
            );
            observed = counts;
            if before == state.counters_epoch.load(Ordering::Acquire) {
                counters_coherent = true;
                break;
            }
        }
        let (started, completed, failed, active) = observed;
        let intended = state.intended(now);
        let remaining = intended.saturating_sub(started);
        let owed = state.oldest_owed(now);
        ProducerSnapshot {
            name: state.config.name,
            period_us: ns(state.config.period) / 1000,
            planned: (ns(state.window) - 1) / ns(state.config.period) + 1,
            intended,
            started,
            completed,
            failed,
            pending_not_started: if stopped { 0 } else { remaining },
            missed_at_stop: if stopped { remaining } else { 0 },
            active_ordinal: (active != NO_ACTIVE).then_some(active),
            maximum_start_lag_us: state.maximum_start_lag_ns.load(Ordering::Relaxed) / 1000,
            maximum_return_lag_us: state.maximum_return_lag_ns.load(Ordering::Acquire) / 1000,
            oldest_owed_ordinal: owed.map(|(ordinal, _)| ordinal),
            oldest_owed_age_us: owed.map(|(_, elapsed)| ns(elapsed) / 1000),
            deadline_breach: owed
                .and_then(|(ordinal, elapsed)| state.deadline_error(ordinal, elapsed)),
            stop_requested: stopped,
            body_released: state.body_released.load(Ordering::Acquire),
            joined: state.joined.load(Ordering::Acquire),
            first_error: state.first_error.get().cloned(),
            counters_coherent,
        }
    }
    fn join_unstarted(&self) -> Result<(), String> {
        if self.start_sent.load(Ordering::Acquire) {
            return Err("constructor cleanup cannot join a started production body".into());
        }
        self.stop_offering();
        // Constructor has not released ANY start barrier. Worker can only
        // receive StopBeforeStart and drop its body, never execute user work.
        if let Some(thread) = self.thread.lock().unwrap().take() {
            thread
                .join()
                .map_err(|_| "unstarted producer cleanup thread panicked")?;
            self.state.joined.store(true, Ordering::Release);
        }
        Ok(())
    }
}
impl ProducerHandle for ActualProducer {
    fn kind(&self) -> &'static str {
        self.state.config.name
    }
    fn start_at(&self, start: Instant) -> Result<(), String> {
        if self.start_sent.swap(true, Ordering::AcqRel) {
            return Err("producer may be started only once".into());
        }
        if self.state.stop_requested.load(Ordering::Acquire) {
            return Err("producer was stopped before start".into());
        }
        self.state
            .start
            .set(start)
            .map_err(|_| "duplicate producer start")?;
        self.start_command
            .send(Command::Start(start))
            .map_err(|error| error.to_string())
    }
    fn stop_offering(&self) {
        self.state.stop(Instant::now());
        if !self.start_sent.load(Ordering::Acquire) {
            let _ = self.start_command.send(Command::StopBeforeStart);
        }
    }
    fn joined(&self) -> bool {
        if self.state.joined.load(Ordering::Acquire) {
            return true;
        }
        let mut slot = self.thread.lock().unwrap();
        if slot.as_ref().is_none_or(|thread| !thread.is_finished()) {
            return false;
        }
        if let Some(thread) = slot.take() {
            if thread.join().is_err() {
                self.state
                    .fail("producer infrastructure thread panicked outside body".into());
            }
            // Actual OS thread joined; body and its captured fixture references
            // have already dropped. The handle itself never stores the body.
            self.state.joined.store(true, Ordering::Release);
        }
        true
    }
}

pub struct ConcreteProducers {
    pub handles: [Arc<ActualProducer>; 4],
}
impl ConcreteProducers {
    pub fn new(
        name: TrialName,
        registry: Arc<WriteWaitRegistry>,
        bodies: ProductionBodies,
    ) -> Result<Self, String> {
        Self::with_window(name, registry, bodies, WINDOW)
    }
    pub fn pipeline_control(
        name: TrialName,
        registry: Arc<WriteWaitRegistry>,
        bodies: ProductionBodies,
    ) -> Result<Self, String> {
        Self::with_window(name, registry, bodies, Duration::from_secs(2))
    }
    pub fn fault_control(
        name: TrialName,
        registry: Arc<WriteWaitRegistry>,
        bodies: ProductionBodies,
        window: Duration,
    ) -> Result<Self, String> {
        if window != Duration::from_secs(1) {
            return Err("only explicit one-second overload producer window accepted".into());
        }
        Self::with_window(name, registry, bodies, window)
    }
    fn with_window(
        name: TrialName,
        registry: Arc<WriteWaitRegistry>,
        bodies: ProductionBodies,
        window: Duration,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| {
            format!("actual producer construction requires Tokio runtime: {error}")
        })?;
        let ProductionBodies {
            status_observations,
            delivery_consumption,
            source_git_refresh,
            watcher_publication,
        } = bodies;
        let mut created = Vec::new();
        for (config, body) in CONFIG.into_iter().zip([
            status_observations,
            delivery_consumption,
            source_git_refresh,
            watcher_publication,
        ]) {
            match ActualProducer::new(
                name,
                config,
                window,
                Arc::clone(&registry),
                runtime.clone(),
                body,
            ) {
                Ok(handle) => created.push(handle),
                Err(error) => {
                    for handle in &created {
                        handle.stop_offering();
                    }
                    let mut cleanup_errors = Vec::new();
                    for handle in &created {
                        if let Err(cleanup) = handle.join_unstarted() {
                            cleanup_errors.push(cleanup);
                        }
                    }
                    return Err(format!(
                        "producer construction: {error}; cleanup: {cleanup_errors:?}"
                    ));
                }
            }
        }
        Ok(Self {
            handles: created
                .try_into()
                .map_err(|_| "exactly four producers required")?,
        })
    }
    pub fn trial_handles(&self) -> ProducerSet {
        ProducerSet(std::array::from_fn(|i| {
            Arc::clone(&self.handles[i]) as Arc<dyn ProducerHandle>
        }))
    }
    pub fn snapshots(&self) -> [ProducerSnapshot; 4] {
        std::array::from_fn(|i| self.handles[i].snapshot())
    }
}

// Integration: copy actual bodies from producers.rs into these four owned
// closures, using ordinal for deterministic active-workspace/owner selection.
// Status calls actual MeasuredStatusPump::observe; delivery calls the real
// DeliveryRunner; source calls actual request_source_sync + sync_sources;
// watcher writes the real private file and observes actual reconciliation and
// publication. Callback completion counters never claim nonempty deliveries,
// matching source completion or watcher publication without their real oracles.
// Production context propagation/guards still need their separate GREEN tests.
