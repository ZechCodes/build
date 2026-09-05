//! What one frame spent its time on.
//!
//! Four durations describe every request the bridge answers: how long it waited
//! for a worker, how long it waited for the app mutex, how long it held it, and
//! how long the whole thing took. They are measured in one place — nothing on
//! the frame path calls `Instant::now` for itself — because the numbers only
//! mean something together: a `board.list` that took a second because it queued
//! behind seven others is a different daemon from one that took a second inside
//! the lock.
//!
//! [`FrameClock`] is that place. The relay hands it a frame when it joins the
//! dispatch queue; the worker starts the frame's [`FrameTimer`] when it picks it
//! up; every acquisition of the app mutex on the frame path goes through
//! [`FrameTimer::lock`], which is the only thing that knows a hold has begun.
//! When the timer drops, the frame's record is published: one histogram bucket
//! per method, and — for a frame over [`SLOW_FRAME`] — one line saying where the
//! time went.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// A frame that takes longer than this, end to end and queue wait included, is
/// worth one line of stderr on its own. Below it the histograms are the record;
/// above it a human is already waiting and wants to know which of the four
/// durations to blame.
pub const SLOW_FRAME: Duration = Duration::from_millis(200);

/// How many distinct methods keep a histogram of their own. The method name
/// comes off the wire, so the map needs a bound; past it every further method
/// is recorded together under [`OVERFLOW_METHOD`]. The bridge answers a few
/// dozen verbs, so reaching this at all means a client is inventing names.
const MAX_TRACKED_METHODS: usize = 128;

/// Where the methods past [`MAX_TRACKED_METHODS`] are counted.
const OVERFLOW_METHOD: &str = "other";

/// The upper edge of each histogram bucket, in microseconds. Fixed and shared
/// by every method, so a method's whole record is one array of counters
/// allocated once, the first time that method is seen.
const BUCKET_CEILINGS_MICROS: [u64; 18] = [
    100,
    250,
    500,
    1_000,
    2_500,
    5_000,
    10_000,
    25_000,
    50_000,
    100_000,
    200_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    10_000_000,
    30_000_000,
    u64::MAX,
];

/// Where a slow frame's line goes: stderr in the daemon, a buffer a test can
/// read back under test.
pub type SlowFrameSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Every frame's timing, since boot.
///
/// Shared as an `Arc` by the relay's dispatcher (which counts the queue) and
/// the frames themselves (which record what they did). Answers `bridge.stats`
/// on its own, holding nothing but its own leaves — a wedged daemon must still
/// be able to say what is wedging it.
pub struct FrameClock {
    methods: RwLock<HashMap<String, Arc<MethodRecord>>>,
    holder: Mutex<Option<Arc<MethodRecord>>>,
    queued: AtomicUsize,
    served: AtomicU64,
    slow: AtomicU64,
    sink: SlowFrameSink,
}

impl FrameClock {
    /// The daemon's clock: slow frames go to stderr.
    pub fn new() -> Arc<FrameClock> {
        FrameClock::reporting_to(Arc::new(|line: &str| eprintln!("{line}")))
    }

    /// A clock whose slow-frame lines go somewhere a test can read them.
    pub fn reporting_to(sink: SlowFrameSink) -> Arc<FrameClock> {
        Arc::new(FrameClock {
            methods: RwLock::new(HashMap::new()),
            holder: Mutex::new(None),
            queued: AtomicUsize::new(0),
            served: AtomicU64::new(0),
            slow: AtomicU64::new(0),
            sink,
        })
    }

    /// A frame has joined the dispatch queue. It counts against the queue depth
    /// from here until a worker starts it — or until the ticket is dropped,
    /// which is what a read folded into an identical one does.
    pub fn queued(self: &Arc<Self>) -> QueuedFrame {
        self.queued.fetch_add(1, Ordering::Relaxed);
        QueuedFrame {
            clock: Arc::clone(self),
            since: Instant::now(),
            counted: true,
        }
    }

    /// A frame that never queued: the relay's session-close frame, and the
    /// direct calls tests make.
    pub fn frame(self: &Arc<Self>, method: &str) -> FrameTimer {
        self.queued().start(method)
    }

    /// The counters `bridge.stats` answers with.
    pub fn stats(&self) -> Value {
        let methods = self.methods.read().unwrap();
        // `serde_json::Map` is a `BTreeMap` here, so the reply reads in method
        // order however the histograms were first inserted.
        let per_method: serde_json::Map<String, Value> = methods
            .iter()
            .map(|(name, record)| (name.clone(), record.stats()))
            .collect();
        json!({
            "frames_served": self.served.load(Ordering::Relaxed),
            "slow_frames": self.slow.load(Ordering::Relaxed),
            "queue_depth": self.queued.load(Ordering::Relaxed),
            "lock_holder": match self.holder.lock().unwrap().as_ref() {
                Some(record) => Value::String(record.method.clone()),
                None => Value::Null,
            },
            "methods": per_method,
        })
    }

    fn record_of(&self, method: &str) -> Arc<MethodRecord> {
        if let Some(record) = self.methods.read().unwrap().get(method) {
            return Arc::clone(record);
        }
        let mut methods = self.methods.write().unwrap();
        let name = if methods.contains_key(method) || methods.len() < MAX_TRACKED_METHODS {
            method
        } else {
            OVERFLOW_METHOD
        };
        Arc::clone(
            methods
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(MethodRecord::new(name))),
        )
    }

    fn hold_began(&self, method: &Arc<MethodRecord>) {
        *self.holder.lock().unwrap() = Some(Arc::clone(method));
    }

    fn hold_ended(&self) {
        *self.holder.lock().unwrap() = None;
    }

    fn publish(&self, frame: &FrameTimer) {
        let spent = frame.spent();
        frame.method.record(spent.total);
        self.served.fetch_add(1, Ordering::Relaxed);
        if spent.total >= SLOW_FRAME {
            self.slow.fetch_add(1, Ordering::Relaxed);
            (self.sink)(&slow_frame_line(
                &frame.method.method,
                &spent,
                self.queued.load(Ordering::Relaxed),
            ));
        }
    }
}

/// The four durations one frame spent.
struct Spent {
    queued: Duration,
    lock_wait: Duration,
    held: Duration,
    total: Duration,
}

/// The one line a slow frame writes.
fn slow_frame_line(method: &str, spent: &Spent, queue_depth: usize) -> String {
    format!(
        "slow frame {method} total={} queued={} lock_wait={} held={} waiting={queue_depth}",
        millis(spent.total),
        millis(spent.queued),
        millis(spent.lock_wait),
        millis(spent.held),
    )
}

fn millis(duration: Duration) -> String {
    format!("{:.1}ms", duration.as_secs_f64() * 1000.0)
}

/// A frame's place in the dispatch queue: what it costs the depth count, and
/// when it started waiting.
pub struct QueuedFrame {
    clock: Arc<FrameClock>,
    since: Instant,
    counted: bool,
}

impl QueuedFrame {
    /// A worker took the frame. The queue wait ends here and the frame's own
    /// record begins.
    pub fn start(mut self, method: &str) -> FrameTimer {
        self.counted = false;
        self.clock.queued.fetch_sub(1, Ordering::Relaxed);
        FrameTimer {
            method: self.clock.record_of(method),
            clock: Arc::clone(&self.clock),
            queued: self.since.elapsed(),
            started: Instant::now(),
            lock_wait_micros: AtomicU64::new(0),
            held_micros: AtomicU64::new(0),
        }
    }
}

impl Drop for QueuedFrame {
    fn drop(&mut self) {
        if self.counted {
            self.clock.queued.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// One frame's record, published when it drops.
///
/// Every acquisition of the app mutex the frame makes goes through
/// [`FrameTimer::lock`], so `lock_wait` and `held` are sums across the frame's
/// several acquisitions, not just its first.
pub struct FrameTimer {
    clock: Arc<FrameClock>,
    method: Arc<MethodRecord>,
    queued: Duration,
    started: Instant,
    lock_wait_micros: AtomicU64,
    held_micros: AtomicU64,
}

impl FrameTimer {
    /// Take the app mutex, timing the wait and the hold.
    ///
    /// The returned guard derefs to the state. While it lives, `bridge.stats`
    /// names this frame's method as the lock holder.
    pub fn lock<'a, T>(&'a self, state: &'a Arc<Mutex<T>>) -> LockedFor<'a, T> {
        let asked_at = Instant::now();
        let guard = state.lock().unwrap();
        self.lock_wait_micros
            .fetch_add(micros(asked_at.elapsed()), Ordering::Relaxed);
        self.clock.hold_began(&self.method);
        LockedFor {
            guard: Some(guard),
            timer: self,
            acquired_at: Instant::now(),
        }
    }

    /// The clock this frame is timed by — where `bridge.stats` reads its
    /// counters from, so the verb that says what is wedging the daemon never
    /// touches the state the wedge is holding.
    pub fn clock(&self) -> &Arc<FrameClock> {
        &self.clock
    }

    fn spent(&self) -> Spent {
        Spent {
            queued: self.queued,
            lock_wait: Duration::from_micros(self.lock_wait_micros.load(Ordering::Relaxed)),
            held: Duration::from_micros(self.held_micros.load(Ordering::Relaxed)),
            total: self.queued + self.started.elapsed(),
        }
    }
}

impl Drop for FrameTimer {
    fn drop(&mut self) {
        self.clock.publish(self);
    }
}

/// The app mutex, held by a frame that is being timed.
///
/// The guard sits in an `Option` so [`Drop`] can release it in the right order:
/// clear the holder slot while the mutex is still held (or the next frame's
/// claim would be wiped by this one's release), drop the guard, and only then
/// write the hold time down.
pub struct LockedFor<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    timer: &'a FrameTimer,
    acquired_at: Instant,
}

impl<'a, T> LockedFor<'a, T> {
    /// Release the mutex, wait for `condvar` until `ready` holds or `timeout`
    /// expires, and take it again.
    ///
    /// The wait is the one way a frame gives the mutex back without ending its
    /// hold, so it is also the one way the frame's four durations could lie: it
    /// closes the hold before waiting and charges the reacquisition to lock
    /// wait, which is what the wait actually is.
    pub fn wait_until(
        mut self,
        condvar: &Condvar,
        timeout: Duration,
        mut ready: impl FnMut(&mut T) -> bool,
    ) -> LockedFor<'a, T> {
        let guard = self.guard.take().expect("held until drop");
        self.timer.clock.hold_ended();
        self.timer
            .held_micros
            .fetch_add(micros(self.acquired_at.elapsed()), Ordering::Relaxed);
        let asked_at = Instant::now();
        let (guard, _) = condvar
            .wait_timeout_while(guard, timeout, |state| !ready(state))
            .expect("the app mutex is never poisoned by a waiter");
        self.timer
            .lock_wait_micros
            .fetch_add(micros(asked_at.elapsed()), Ordering::Relaxed);
        self.timer.clock.hold_began(&self.timer.method);
        self.acquired_at = Instant::now();
        self.guard = Some(guard);
        self
    }
}

impl<T> std::ops::Deref for LockedFor<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_ref().expect("held until drop")
    }
}

impl<T> std::ops::DerefMut for LockedFor<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("held until drop")
    }
}

impl<T> Drop for LockedFor<'_, T> {
    fn drop(&mut self) {
        self.timer.clock.hold_ended();
        self.guard.take();
        self.timer
            .held_micros
            .fetch_add(micros(self.acquired_at.elapsed()), Ordering::Relaxed);
    }
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

/// One method's frames since boot: how many, how slow the slowest was, and a
/// fixed histogram of the rest.
struct MethodRecord {
    method: String,
    served: AtomicU64,
    max_micros: AtomicU64,
    buckets: [AtomicU64; BUCKET_CEILINGS_MICROS.len()],
}

impl MethodRecord {
    fn new(method: &str) -> MethodRecord {
        MethodRecord {
            method: method.to_string(),
            served: AtomicU64::new(0),
            max_micros: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn record(&self, total: Duration) {
        let total = micros(total);
        self.served.fetch_add(1, Ordering::Relaxed);
        self.max_micros.fetch_max(total, Ordering::Relaxed);
        let bucket = BUCKET_CEILINGS_MICROS
            .iter()
            .position(|ceiling| total <= *ceiling)
            .unwrap_or(BUCKET_CEILINGS_MICROS.len() - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// The bucket ceiling the given share of frames falls at or below, capped
    /// by the largest frame actually seen — the buckets are coarse, and a
    /// percentile above the maximum would be a number no frame ever took.
    fn quantile_micros(&self, share: f64) -> u64 {
        let served = self.served.load(Ordering::Relaxed);
        if served == 0 {
            return 0;
        }
        let max = self.max_micros.load(Ordering::Relaxed);
        let target = ((served as f64) * share).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (bucket, ceiling) in self.buckets.iter().zip(BUCKET_CEILINGS_MICROS) {
            seen += bucket.load(Ordering::Relaxed);
            if seen >= target {
                return ceiling.min(max);
            }
        }
        max
    }

    fn stats(&self) -> Value {
        json!({
            "served": self.served.load(Ordering::Relaxed),
            "p50_ms": as_millis(self.quantile_micros(0.50)),
            "p95_ms": as_millis(self.quantile_micros(0.95)),
            "max_ms": as_millis(self.max_micros.load(Ordering::Relaxed)),
        })
    }
}

fn as_millis(micros: u64) -> f64 {
    (micros as f64) / 1000.0
}

/// A clock plus the slow-frame lines it has written, for the tests of every
/// module that watches what the clock reports.
#[cfg(test)]
pub(crate) fn recording_clock() -> (Arc<FrameClock>, Arc<Mutex<Vec<String>>>) {
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let clock = FrameClock::reporting_to(Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    }));
    (clock, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_counted_under_its_own_method() {
        let (clock, _) = recording_clock();
        let state = Arc::new(Mutex::new(0u32));
        {
            let timer = clock.frame("board.list");
            *timer.lock(&state) += 1;
        }
        let stats = clock.stats();
        assert_eq!(stats["frames_served"], 1);
        assert_eq!(stats["methods"]["board.list"]["served"], 1);
        assert_eq!(stats["queue_depth"], 0);
    }

    #[test]
    fn a_slow_frame_logs_its_four_durations() {
        let (clock, lines) = recording_clock();
        let state = Arc::new(Mutex::new(0u32));

        let queued = clock.queued();
        std::thread::sleep(Duration::from_millis(10));
        let held_by_another = Arc::clone(&state);
        let holder = std::thread::spawn(move || {
            let guard = held_by_another.lock().unwrap();
            std::thread::sleep(Duration::from_millis(220));
            drop(guard);
        });
        std::thread::sleep(Duration::from_millis(20));
        {
            let timer = queued.start("worktree.create");
            *timer.lock(&state) += 1;
        }
        holder.join().unwrap();

        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "one line per slow frame: {lines:?}");
        let line = &lines[0];
        assert!(line.starts_with("slow frame worktree.create "), "{line}");
        for label in ["total=", "queued=", "lock_wait=", "held=", "waiting="] {
            assert!(line.contains(label), "{label} missing from {line}");
        }
        assert_eq!(clock.stats()["slow_frames"], 1);
    }

    #[test]
    fn a_quick_frame_logs_nothing() {
        let (clock, lines) = recording_clock();
        let state = Arc::new(Mutex::new(0u32));
        {
            let timer = clock.frame("board.list");
            *timer.lock(&state) += 1;
        }
        assert!(lines.lock().unwrap().is_empty());
        assert_eq!(clock.stats()["slow_frames"], 0);
    }

    #[test]
    fn stats_name_the_method_holding_the_state_lock() {
        let (clock, _) = recording_clock();
        let state = Arc::new(Mutex::new(0u32));
        let timer = clock.frame("thread.post");
        let held = timer.lock(&state);
        assert_eq!(clock.stats()["lock_holder"], "thread.post");
        drop(held);
        assert_eq!(clock.stats()["lock_holder"], Value::Null);
    }

    #[test]
    fn a_queued_frame_counts_against_the_depth_until_it_starts() {
        let (clock, _) = recording_clock();
        let first = clock.queued();
        let second = clock.queued();
        assert_eq!(clock.stats()["queue_depth"], 2);
        let timer = first.start("board.list");
        assert_eq!(clock.stats()["queue_depth"], 1);
        drop(second);
        assert_eq!(clock.stats()["queue_depth"], 0);
        drop(timer);
    }

    #[test]
    fn a_frames_lock_time_is_summed_across_its_acquisitions() {
        let (clock, _) = recording_clock();
        let state = Arc::new(Mutex::new(0u32));
        let timer = clock.frame("run.create");
        for _ in 0..3 {
            let mut held = timer.lock(&state);
            *held += 1;
            std::thread::sleep(Duration::from_millis(5));
        }
        let spent = timer.spent();
        assert!(
            spent.held >= Duration::from_millis(15),
            "held {:?} should cover all three acquisitions",
            spent.held
        );
        assert!(spent.total >= spent.held);
    }

    #[test]
    fn a_frame_waiting_on_a_condvar_holds_nothing_and_charges_the_wait_to_the_lock() {
        let (clock, _) = recording_clock();
        let state = Arc::new(Mutex::new(false));
        let woken = Arc::new(Condvar::new());

        let waker_state = Arc::clone(&state);
        let waker = Arc::clone(&woken);
        let sampling_clock = Arc::clone(&clock);
        let holder_while_waiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let seen = sampling_clock.stats()["lock_holder"].clone();
            *waker_state.lock().unwrap() = true;
            waker.notify_all();
            seen
        });

        let timer = clock.frame("agent.deliver");
        let held = timer
            .lock(&state)
            .wait_until(&woken, Duration::from_secs(5), |ready| *ready);
        assert!(*held, "the waiter woke on the condition, not the timeout");
        drop(held);

        assert_eq!(
            holder_while_waiting.join().unwrap(),
            Value::Null,
            "a waiting frame holds the mutex for nobody"
        );
        assert!(
            timer.spent().lock_wait >= Duration::from_millis(15),
            "the wait is charged to lock wait, not to the hold"
        );
    }

    #[test]
    fn a_flood_of_invented_methods_collapses_into_one_record() {
        let (clock, _) = recording_clock();
        for n in 0..MAX_TRACKED_METHODS + 50 {
            drop(clock.frame(&format!("made.up.{n}")));
        }
        let stats = clock.stats();
        let methods = stats["methods"].as_object().unwrap();
        assert_eq!(methods.len(), MAX_TRACKED_METHODS + 1);
        assert_eq!(methods[OVERFLOW_METHOD]["served"], 50);
    }

    #[test]
    fn quantiles_span_the_recorded_frames() {
        let record = MethodRecord::new("board.list");
        for _ in 0..95 {
            record.record(Duration::from_micros(400));
        }
        for _ in 0..5 {
            record.record(Duration::from_millis(900));
        }
        assert_eq!(record.quantile_micros(0.50), 500);
        assert_eq!(record.quantile_micros(0.95), 500);
        assert_eq!(record.stats()["max_ms"], 900.0);
    }

    #[test]
    fn a_method_nobody_has_called_reports_nothing() {
        let (clock, _) = recording_clock();
        let stats = clock.stats();
        assert_eq!(stats["frames_served"], 0);
        assert_eq!(stats["lock_holder"], Value::Null);
        assert!(stats["methods"].as_object().unwrap().is_empty());
    }
}
