//! Test-only exact samples; no production metrics or labels are added.
//! Phase durations are exclusive: nested work is charged to its deepest span.
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
