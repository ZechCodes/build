//! Test-only exact samples; no production metrics or labels are added.
//! Phase durations are exclusive: nested work is charged to its deepest span.
use super::*;
use std::cell::RefCell;
use std::collections::BTreeMap;

type Measurements = BTreeMap<String, BTreeMap<String, Vec<f64>>>;

#[derive(Clone, Default)]
pub(crate) struct PhaseSamples(Arc<Mutex<Measurements>>);

impl PhaseSamples {
    fn record(&self, method: &str, phase: &str, duration: Duration) {
        self.0
            .lock()
            .unwrap()
            .entry(method.into())
            .or_default()
            .entry(phase.into())
            .or_default()
            .push(duration.as_secs_f64() * 1000.0);
    }

    pub(crate) fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    pub(crate) fn snapshot(&self) -> Value {
        let mut methods = serde_json::Map::new();
        for (method, phases) in self.0.lock().unwrap().iter() {
            let mut rows = serde_json::Map::new();
            for (phase, values) in phases {
                let mut values = values.clone();
                values.sort_by(f64::total_cmp);
                let n = values.len();
                rows.insert(phase.clone(), json!({"count":n,"sum_ms":values.iter().sum::<f64>(),
                    "p50_ms":values[(n*50).div_ceil(100)-1],"p95_ms":values[(n*95).div_ceil(100)-1],"max_ms":values[n-1]}));
            }
            methods.insert(method.clone(), Value::Object(rows));
        }
        Value::Object(methods)
    }
}

impl FrameClock {
    pub(crate) fn capture_phases(&self) -> PhaseSamples {
        let samples = PhaseSamples::default();
        *self.phase_samples.lock().unwrap() = Some(samples.clone());
        samples
    }
}

impl FrameTimer {
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

pub(super) fn record_frame(timer: &FrameTimer) {
    if let Some(samples) = timer.clock.phase_samples.lock().unwrap().as_ref() {
        let spent = timer.spent();
        let method = timer.sample_label();
        for (name, elapsed) in [
            ("frame.held", spent.held),
            ("frame.lock_wait", spent.lock_wait),
            ("frame.queued", spent.queued),
            ("frame.total", spent.total),
        ] {
            samples.record(&method, name, elapsed);
        }
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
        let samples = timer.clock.phase_samples.lock().unwrap().clone()?;
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
