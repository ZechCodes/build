//! Identical test-only endpoints for old/new status-pump guard measurements.
//! The collector is thread local: no observer mutex or callback runs under AppState.

use super::*;
use crate::harness::SessionStatusSnapshot;
use std::cell::{Cell, RefCell};
use std::time::Instant;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static SAMPLE: RefCell<Option<GuardSample>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
pub(in crate::app) struct GuardSample {
    pub wait: Duration,
    pub held: Duration,
    pub observe: Duration,
}

pub(super) struct GuardSpan {
    waiting: Instant,
    acquired: Option<Instant>,
}

impl GuardSpan {
    pub(super) fn start() -> Option<Self> {
        ENABLED.with(|enabled| enabled.get()).then(|| Self {
            waiting: Instant::now(),
            acquired: None,
        })
    }

    pub(super) fn acquired(&mut self) {
        self.acquired = Some(Instant::now());
    }
}

impl Drop for GuardSpan {
    fn drop(&mut self) {
        let released = Instant::now();
        let acquired = self.acquired.expect("status guard was acquired");
        SAMPLE.with(|sample| {
            *sample.borrow_mut() = Some(GuardSample {
                wait: acquired.duration_since(self.waiting),
                held: released.duration_since(acquired),
                observe: Duration::ZERO,
            });
        });
    }
}

/// Drive the same private pump synchronously, avoiding watch coalescing in a
/// latency fixture. The live tab and session-instance checks remain production code.
pub(in crate::app) struct MeasuredStatusPump(StatusPump);

impl MeasuredStatusPump {
    pub(in crate::app) fn new(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        session: &Arc<dyn AgentSession>,
        instance: SessionInstance,
    ) -> Self {
        Self(StatusPump {
            state: Arc::downgrade(state),
            key,
            session: Arc::downgrade(session),
            instance,
            frame_clock: Some(Arc::clone(&state.lock().unwrap().frame_clock)),
            recorded_context: None,
            recorded_context_reset_count: 0,
            recorded_limit: super::super::usage_limits::UsageObservation::default(),
        })
    }

    pub(in crate::app) fn observe(&mut self, snapshot: &SessionStatusSnapshot) -> GuardSample {
        SAMPLE.with(|sample| assert!(sample.borrow().is_none()));
        ENABLED.with(|enabled| assert!(!enabled.replace(true)));
        let started = Instant::now();
        let live = self.0.observe(snapshot);
        let elapsed = started.elapsed();
        ENABLED.with(|enabled| enabled.set(false));
        assert!(live, "the benchmark keeps the exact status session live");
        let mut sample = SAMPLE.with(|sample| sample.borrow_mut().take()).unwrap();
        sample.observe = elapsed;
        sample
    }
}
