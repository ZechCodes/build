//! Scoped evidence of actual native contention attempts, separate from timing.
use super::{IntervalCategory, WaitId, WaitKind};
use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

thread_local! {
    static OBSERVER: RefCell<Option<Sender<Attempt>>> = const { RefCell::new(None) };
}

pub(crate) struct Attempt {
    pub(crate) label: &'static str,
    pub(crate) at: Instant,
    pub(crate) intervals: Vec<WaitId>,
}

pub(crate) struct Observer {
    previous: Option<Sender<Attempt>>,
    not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Observer {
    pub(crate) fn bind(sender: Sender<Attempt>) -> Self {
        Self {
            previous: OBSERVER.with(|current| current.replace(Some(sender))),
            not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        OBSERVER.with(|current| current.replace(self.previous.take()));
    }
}

/// Called only after a real native contention result selects a retry.
pub(crate) fn observed(label: &'static str) {
    let Some(sender) = OBSERVER.with(|current| current.borrow().clone()) else {
        return;
    };
    let intervals = super::current_context()
        .map(|context| {
            context
                .registry
                .active_snapshot()
                .into_iter()
                .filter(|row| context.origins().contains(&row.id))
                .flat_map(|row| row.active)
                .filter(|wait| wait.category == IntervalCategory::KnownWait(WaitKind::ResourceLock))
                .map(|wait| wait.id)
                .collect()
        })
        .unwrap_or_default();
    let _ = sender.send(Attempt {
        label,
        at: Instant::now(),
        intervals,
    });
}

/// Return partial evidence on timeout so the caller can release native locks
/// and join before deciding whether the contention oracle passed.
pub(crate) fn collect(receiver: &Receiver<Attempt>, count: usize) -> Vec<Attempt> {
    let mut attempts = Vec::new();
    while attempts.len() < count {
        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(attempt) => attempts.push(attempt),
            Err(_) => break,
        }
    }
    attempts
}

pub(crate) fn assert_continuous(
    tracked: &super::deep_test_support::TrackedWrite,
    attempts: &[Attempt],
    label: &'static str,
) {
    assert!(
        attempts.len() >= 3,
        "fewer than three actual retries: {label}"
    );
    let first = &attempts[0];
    assert_eq!(
        first.intervals.len(),
        1,
        "missing continuous retry interval: {label}"
    );
    for attempt in attempts {
        assert_eq!(attempt.label, label);
        assert_eq!(
            attempt.intervals, first.intervals,
            "retry split or reset its wait"
        );
    }
    let intervals = tracked.registry.completed_intervals();
    let interval = intervals
        .iter()
        .find(|interval| interval.id == first.intervals[0])
        .expect("successful acquisition or timeout must close the same interval");
    assert!(interval.started <= first.at);
    assert!(interval.ended >= attempts.last().unwrap().at);
    assert!(tracked.row().active.is_empty());
}
