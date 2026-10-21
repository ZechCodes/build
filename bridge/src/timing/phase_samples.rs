//! Test-only exact samples; no production metrics or labels are added.
//! Phase durations are exclusive: nested work is charged to its deepest span.
//! Read stage `.total` rows include their descendants and occur once per
//! stage/request. They overlap the exclusive rows, as do the frame/client totals.
//! `read.class_wait` and `read.sql_wait` are async waits outside the read spans;
//! `frame.pool_wait` measures blocking-worker scheduling only. Sums across
//! exclusive spans plus these waits explain the frame, apart from unscoped
//! async dispatch overhead. `reply.serialization` starts after the frame ends.
use super::*;
use std::cell::RefCell;
use std::collections::BTreeMap;

type Measurements = BTreeMap<String, BTreeMap<String, Vec<f64>>>;

#[derive(Clone, Default)]
pub(crate) struct PhaseSamples {
    measurements: Arc<Mutex<Measurements>>,
    active_frames: Arc<AtomicUsize>,
}

impl PhaseSamples {
    fn record(&self, method: &str, phase: &str, duration: Duration) {
        self.measurements
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .entry(phase.into())
            .or_default()
            .push(duration.as_secs_f64() * 1000.0);
    }

    /// Caller-side latency, measured outside FrameTimer through parsed reply
    /// or the joined status observation. This is not an exclusive lock phase.
    pub(crate) fn record_client_total(&self, method: &str, elapsed: Duration) {
        self.record(method, "client.total", elapsed);
    }

    pub(crate) fn active_frames(&self) -> usize {
        self.active_frames.load(Ordering::Acquire)
    }

    pub(crate) fn clear(&self) {
        self.measurements.lock().unwrap().clear();
    }

    pub(crate) fn snapshot(&self) -> Value {
        let captured = self.measurements.lock().unwrap().clone();
        let mut methods = serde_json::Map::new();
        for (method, phases) in captured {
            let mut rows = serde_json::Map::new();
            for (phase, mut values) in phases {
                values.sort_by(f64::total_cmp);
                let n = values.len();
                rows.insert(phase, json!({"count":n,"sum_ms":values.iter().sum::<f64>(),
                    "p50_ms":values[(n*50).div_ceil(100)-1],"p95_ms":values[(n*95).div_ceil(100)-1],"p99_ms":values[(n*99).div_ceil(100)-1],"max_ms":values[n-1]}));
            }
            methods.insert(method, Value::Object(rows));
        }
        Value::Object(methods)
    }
}

impl FrameClock {
    pub(crate) fn sample_serialization<T>(&self, tool: &str, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let value = work();
        if let Some(samples) = self.phase_samples.lock().unwrap().as_ref() {
            samples.record(
                &format!("mcp.control:{tool}"),
                "reply.serialization",
                started.elapsed(),
            );
        }
        value
    }

    pub(crate) fn capture_phases(&self) -> PhaseSamples {
        let samples = PhaseSamples::default();
        *self.phase_samples.lock().unwrap() = Some(samples.clone());
        samples
    }
}

impl FrameTimer {
    /// Record an async read boundary without creating thread-local scopes.
    pub(crate) fn sample_read_duration(&self, phase: &'static str, elapsed: Duration) {
        if let Some(samples) = self.phase_capture.as_ref() {
            samples.record(&self.sample_label(), phase, elapsed);
        }
    }

    /// Capture independent read work without implying an app-lock acquisition.
    pub(crate) fn sample_read_work<T>(&self, work: impl FnOnce() -> T) -> T {
        let Some(samples) = self.phase_capture.clone() else {
            return work();
        };
        ACTIVE.with(|active| {
            assert!(active.borrow().is_none(), "read work cannot nest a capture");
            *active.borrow_mut() = Some(Active {
                samples,
                method: self.sample_label(),
                stack: vec![("read.unclassified", Instant::now(), Duration::ZERO)],
            });
        });
        let _sample = LockSample;
        work()
    }

    pub(crate) fn sample_tool(&self, tool: &str) {
        *self.sample_tool.lock().unwrap() = Some(tool.into());
    }

    fn sample_label(&self) -> String {
        match self.sample_tool.lock().unwrap().as_deref() {
            Some(tool) => format!("{}:{tool}", self.method.method),
            None => self.method.method.clone(),
        }
    }
}

pub(super) fn start_frame(clock: &FrameClock) -> Option<PhaseSamples> {
    let samples = clock.phase_samples.lock().unwrap().clone()?;
    samples.active_frames.fetch_add(1, Ordering::Relaxed);
    Some(samples)
}

pub(super) fn record_frame(timer: &FrameTimer) {
    if let Some(samples) = timer.phase_capture.as_ref() {
        let spent = timer.spent();
        let method = timer.sample_label();
        let permit = spent.permit_wait.unwrap_or_default();
        let pool = spent.pool_wait.unwrap_or_default();
        for (name, elapsed) in [
            ("frame.held", spent.held),
            ("frame.lock_wait", spent.lock_wait),
            ("frame.queued", spent.queued),
            ("frame.permit_wait", permit),
            ("frame.pool_wait", pool),
            (
                "frame.queue_permit_and_lock_wait",
                spent.queued + permit + spent.lock_wait,
            ),
            ("frame.queue_and_lock_wait", spent.queued + spent.lock_wait),
            ("frame.total", spent.total),
        ] {
            samples.record(&method, name, elapsed);
        }
        samples.active_frames.fetch_sub(1, Ordering::Release);
    }
}

struct Active {
    samples: PhaseSamples,
    method: String,
    stack: Vec<(&'static str, Instant, Duration)>,
}
thread_local! { static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) }; }

pub(crate) struct LockSample;
impl LockSample {
    pub(super) fn begin(timer: &FrameTimer) -> Option<Self> {
        let samples = timer.phase_capture.clone()?;
        samples.record(&timer.sample_label(), "entered.lock", Duration::ZERO);
        ACTIVE.with(|active| {
            assert!(active.borrow().is_none(), "app locks cannot nest");
            *active.borrow_mut() = Some(Active {
                samples,
                method: timer.sample_label(),
                stack: vec![("unclassified", Instant::now(), Duration::ZERO)],
            });
        });
        Some(Self)
    }
}
impl Drop for LockSample {
    fn drop(&mut self) {
        end_phase();
        ACTIVE.with(|active| {
            active.borrow_mut().take();
        });
    }
}

pub(crate) struct Phase(bool);
pub(crate) fn phase(name: &'static str) -> Phase {
    let enabled = ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let Some(active) = active.as_mut() else {
            return false;
        };
        active
            .samples
            .record(&active.method, &format!("entered.{name}"), Duration::ZERO);
        active.stack.push((name, Instant::now(), Duration::ZERO));
        true
    });
    Phase(enabled)
}
impl Drop for Phase {
    fn drop(&mut self) {
        if self.0 {
            end_phase();
        }
    }
}

/// One inclusive stage observation per request, alongside its exclusive span.
/// Detailed `sql.*`/snapshot spans subtract from the stage's exclusive row;
/// the `.total` row remains suitable for per-request stage percentiles.
pub(crate) struct Stage {
    phase: Phase,
    name: &'static str,
    started: Instant,
}

pub(crate) fn stage(name: &'static str) -> Stage {
    let started = Instant::now();
    Stage {
        phase: phase(name),
        name,
        started,
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if !self.phase.0 {
            return;
        }
        end_phase();
        self.phase.0 = false;
        let elapsed = self.started.elapsed();
        ACTIVE.with(|active| {
            let active = active.borrow();
            let active = active.as_ref().expect("captured read stage is active");
            active
                .samples
                .record(&active.method, &format!("{}.total", self.name), elapsed);
        });
    }
}

#[test]
fn phase_capture_read_work_does_not_count_app_lock_entries() {
    let (clock, _) = recording_clock();
    let samples = clock.capture_phases();
    let timer = clock.frame("mcp.control");
    timer.sample_tool("list_work");
    let value = timer.sample_read_work(|| {
        let _run = stage("read.run");
        let _materialize = phase("snapshot.materialize");
        42
    });
    // Each blocking stage gets a fresh scope; its thread-local state ends
    // before the next stage starts, and cannot capture unrelated work.
    timer.sample_read_work(|| {
        let _finish = stage("read.finish");
    });
    {
        let _outside = phase("outside");
    }
    assert_eq!(value, 42);
    drop(timer);
    let captured = samples.snapshot();
    let row = &captured["mcp.control:list_work"];
    assert!(row.get("entered.lock").is_none(), "{captured}");
    assert!(row.get("unclassified").is_none(), "{captured}");
    assert!(row.get("outside").is_none(), "{captured}");
    assert_eq!(row["frame.held"]["sum_ms"], 0.0);
    assert_eq!(row["frame.lock_wait"]["sum_ms"], 0.0);
    for name in ["read.run", "snapshot.materialize", "read.finish"] {
        assert_eq!(row[name]["count"], 1, "{captured}");
        assert_eq!(row[format!("entered.{name}")]["count"], 1, "{captured}");
    }
    assert_eq!(row["read.unclassified"]["count"], 2);
    for (stage, descendants) in [
        ("read.run", vec!["read.run", "snapshot.materialize"]),
        ("read.finish", vec!["read.finish"]),
    ] {
        let total = &row[format!("{stage}.total")];
        assert_eq!(total["count"], 1);
        let exclusive = descendants
            .into_iter()
            .map(|name| row[name]["sum_ms"].as_f64().unwrap())
            .sum::<f64>();
        assert!(exclusive <= total["sum_ms"].as_f64().unwrap());
    }
    let exclusive = [
        "read.unclassified",
        "read.run",
        "snapshot.materialize",
        "read.finish",
    ]
    .into_iter()
    .map(|name| row[name]["sum_ms"].as_f64().unwrap())
    .sum::<f64>();
    assert!(exclusive <= row["frame.total"]["sum_ms"].as_f64().unwrap());
}

#[test]
fn phase_capture_duration_uses_frame_collector_and_tool_label() {
    let (clock, _) = recording_clock();
    let original = clock.capture_phases();
    let timer = clock.frame("mcp.control");
    timer.sample_tool("get_task");
    let replacement = clock.capture_phases();
    timer.sample_read_duration("read.sql_wait", Duration::from_millis(7));
    timer.sample_read_duration("read.class_wait", Duration::from_millis(3));
    drop(timer);
    let captured = original.snapshot();
    let row = &captured["mcp.control:get_task"];
    assert_eq!(row["read.sql_wait"]["count"], 1);
    assert_eq!(row["read.sql_wait"]["sum_ms"], 7.0);
    assert_eq!(row["read.class_wait"]["count"], 1);
    assert_eq!(row["read.class_wait"]["sum_ms"], 3.0);
    assert!(row.get("entered.lock").is_none());
    assert_eq!(replacement.snapshot(), json!({}));
}

#[test]
fn phase_capture_percentiles_use_nearest_rank() {
    let samples = PhaseSamples::default();
    for millis in (1..=100).rev() {
        samples.record(
            "mcp.control:list_work",
            "client.total",
            Duration::from_millis(millis),
        );
    }
    let captured = samples.snapshot();
    let row = &captured["mcp.control:list_work"]["client.total"];
    assert_eq!(row["count"], 100);
    assert_eq!(row["sum_ms"], 5050.0);
    assert_eq!(row["p50_ms"], 50.0);
    assert_eq!(row["p95_ms"], 95.0);
    assert_eq!(row["p99_ms"], 99.0);
    assert_eq!(row["max_ms"], 100.0);
}

#[test]
fn phase_capture_serialization_records_tool_and_returns_the_value() {
    let (clock, _) = recording_clock();
    let samples = clock.capture_phases();
    let value = clock.sample_serialization("list_work", || String::from("reply"));
    assert_eq!(value, "reply");
    let captured = samples.snapshot();
    let row = &captured["mcp.control:list_work"];
    assert_eq!(row["reply.serialization"]["count"], 1);
    assert!(row.get("entered.lock").is_none());
}

#[test]
fn phase_capture_keeps_in_flight_frames_with_their_original_collector() {
    let (clock, _) = recording_clock();
    let uncaptured = clock.frame("git.show");
    let first = clock.capture_phases();
    let old = clock.frame("mcp.control");
    let second = clock.capture_phases();
    let new = clock.frame("board.list");
    assert_eq!(first.active_frames(), 1);
    assert_eq!(second.active_frames(), 1);
    let state = Arc::new(Mutex::new(()));
    {
        let _guard = old.lock(&state);
        let _phase = phase("original");
    }
    drop(uncaptured);
    drop(old);
    assert_eq!(first.active_frames(), 0);
    assert_eq!(second.active_frames(), 1);
    assert_eq!(first.snapshot()["mcp.control"]["original"]["count"], 1);
    assert!(second.snapshot().get("mcp.control").is_none());
    drop(new);
    assert_eq!(second.active_frames(), 0);
    assert_eq!(second.snapshot()["board.list"]["frame.held"]["count"], 1);
    assert!(first.snapshot().get("git.show").is_none());
    assert!(second.snapshot().get("git.show").is_none());
}
fn end_phase() {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let active = active.as_mut().expect("captured lock is held");
        let (name, started, children) = active.stack.pop().unwrap();
        let elapsed = started.elapsed();
        active
            .samples
            .record(&active.method, name, elapsed.saturating_sub(children));
        if let Some((_, _, parent_children)) = active.stack.last_mut() {
            *parent_children += elapsed;
        }
    });
}

#[test]
fn phase_capture_attributes_only_work_while_the_timed_lock_is_held() {
    let (clock, _) = recording_clock();
    let samples = clock.capture_phases();
    let state = Arc::new(Mutex::new(()));
    let timer = clock.frame("mcp.control");
    timer.sample_tool("get_task");
    {
        let _outside = phase("outside");
        let _guard = timer.lock(&state);
        let _outer = phase("outer");
        let _inner = phase("inner");
    }
    drop(timer);
    let captured = samples.snapshot();
    let method = &captured["mcp.control:get_task"];
    assert!(method.get("outside").is_none());
    for name in ["inner", "outer", "unclassified", "frame.held"] {
        assert_eq!(method[name]["count"], 1, "{captured}");
    }
    let total = ["inner", "outer", "unclassified"]
        .into_iter()
        .map(|name| method[name]["sum_ms"].as_f64().unwrap())
        .sum::<f64>();
    assert!(
        total <= method["frame.held"]["sum_ms"].as_f64().unwrap() + 0.01,
        "exclusive spans cannot double-count work: {captured}"
    );
}

#[test]
fn phase_capture_records_permit_wait_and_combines_the_same_frame_only() {
    let (clock, _) = recording_clock();
    let samples = clock.capture_phases();
    for (queued_ms, permit_ms, lock_ms) in [(10, 1, 3), (1, 7, 1)] {
        let mut timer = clock.frame("mcp.control");
        timer.sample_tool("get_task");
        // Fixed counter inputs test the collector arithmetic, not host timing.
        timer.queued = Duration::from_millis(queued_ms);
        timer.record_permit_wait(Duration::from_millis(permit_ms));
        timer
            .lock_wait_micros
            .store(lock_ms * 1000, Ordering::Relaxed);
        drop(timer);
    }
    let captured = samples.snapshot();
    let row = &captured["mcp.control:get_task"];
    for name in [
        "frame.held",
        "frame.queued",
        "frame.lock_wait",
        "frame.permit_wait",
        "frame.pool_wait",
        "frame.queue_permit_and_lock_wait",
        "frame.total",
    ] {
        assert_eq!(row[name]["count"], 2);
    }
    assert_eq!(row["frame.permit_wait"]["max_ms"], 7.0);
    assert_eq!(row["frame.queue_permit_and_lock_wait"]["max_ms"], 14.0);
    let independent_maxima = row["frame.queued"]["max_ms"].as_f64().unwrap()
        + row["frame.permit_wait"]["max_ms"].as_f64().unwrap()
        + row["frame.lock_wait"]["max_ms"].as_f64().unwrap();
    assert!(
        row["frame.queue_permit_and_lock_wait"]["max_ms"]
            .as_f64()
            .unwrap()
            < independent_maxima
    );
}
