//! Fixed publication stages, independent of request labels and app locks.
//!
//! Prepare covers finisher candidate staging, including owner_prepare, which
//! captures each ordinary owner projection. Persist measures save attempts,
//! including failures; install measures attempts, including rejected stale deltas.
//! Accepted installs atomically publish the snapshot; push covers a nonempty
//! ChangeBus flush through encrypted carrier submission. Push excludes the
//! coalescing wait and does not imply a browser receipt. Spans may nest and
//! include unrelated operations within their boundaries: their sums are not an
//! exclusive breakdown of one event. Drop records only histogram atomics.

use super::{FrameClock, Histogram};
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) enum PublicationPhase {
    Prepare,
    OwnerPrepare,
    Persist,
    Install,
    Push,
}

impl PublicationPhase {
    const ALL: [Self; 5] = [
        Self::Prepare,
        Self::OwnerPrepare,
        Self::Persist,
        Self::Install,
        Self::Push,
    ];

    fn slot(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::OwnerPrepare => "owner_prepare",
            Self::Persist => "persist",
            Self::Install => "install",
            Self::Push => "push",
        }
    }
}

pub(super) struct PublicationTiming {
    phases: [Histogram; 5],
    push_pending: Histogram,
}

impl PublicationTiming {
    pub(super) fn new() -> Self {
        Self {
            phases: std::array::from_fn(|_| Histogram::new()),
            push_pending: Histogram::new(),
        }
    }

    pub(super) fn stats(&self) -> Value {
        Value::Object(
            PublicationPhase::ALL
                .into_iter()
                .map(|phase| (phase.label().to_owned(), self.phases[phase.slot()].stats()))
                .chain(std::iter::once((
                    "push_pending".to_owned(),
                    self.push_pending.stats(),
                )))
                .collect::<Map<_, _>>(),
        )
    }
}

pub(crate) struct PublicationSpan<'a> {
    histogram: &'a Histogram,
    started: Instant,
}

impl FrameClock {
    /// Age of the oldest pending subscription batch through carrier submission.
    /// Includes coalescing and flush work; partial holds and requeue mean this
    /// is not exact per-event latency, and submission is not a client receipt.
    pub(crate) fn record_push_pending(&self, spent: Duration) {
        self.publication.push_pending.record(spent);
    }

    pub(crate) fn publication_phase(&self, phase: PublicationPhase) -> PublicationSpan<'_> {
        PublicationSpan {
            histogram: &self.publication.phases[phase.slot()],
            started: Instant::now(),
        }
    }
}

impl Drop for PublicationSpan<'_> {
    fn drop(&mut self) {
        self.histogram.record(self.started.elapsed());
    }
}
