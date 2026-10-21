//! Deterministic test-only SQL pinning, independent from query implementation.
use crate::test_support::off_lock::{OffLockGate, OffLockGateHandle};
use std::future::{poll_fn, Future};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SqlReadProbeEvent {
    Pinned(&'static str),
    Waiting(&'static str),
}

#[derive(Clone)]
pub(in crate::app) struct SqlReadProbe {
    tools: Arc<[&'static str]>,
    events: mpsc::UnboundedSender<SqlReadProbeEvent>,
    gate: OffLockGate,
}

pub(in crate::app) struct SqlReadProbeHandle {
    events: mpsc::UnboundedReceiver<SqlReadProbeEvent>,
    // Dropping this sender releases every pinned worker, even on assertion
    // failure. Newly admitted requests see its closed channel and continue.
    _gate: OffLockGateHandle,
}

impl SqlReadProbe {
    pub(in crate::app) fn new(tools: Vec<&'static str>) -> (Self, SqlReadProbeHandle) {
        let (events, receiver) = mpsc::unbounded_channel();
        let (gate, handle) = OffLockGate::new();
        (
            Self {
                tools: tools.into(),
                events,
                gate,
            },
            SqlReadProbeHandle {
                events: receiver,
                _gate: handle,
            },
        )
    }

    pub(super) fn pin(&self, tool: &'static str) {
        if self.tools.contains(&tool) {
            let _ = self.events.send(SqlReadProbeEvent::Pinned(tool));
            self.gate.arrive();
        }
    }

    pub(super) async fn observe_wait<T>(
        &self,
        tool: &'static str,
        waiting: impl Future<Output = T>,
    ) -> T {
        let mut reported = false;
        tokio::pin!(waiting);
        poll_fn(|context| {
            let result = waiting.as_mut().poll(context);
            // Report only after acquisition actually returned Pending, once
            // per request. No SQL connection has been requested at this point.
            if result.is_pending() && !reported {
                reported = true;
                if self.tools.contains(&tool) {
                    let _ = self.events.send(SqlReadProbeEvent::Waiting(tool));
                }
            }
            result
        })
        .await
    }
}

impl SqlReadProbeHandle {
    pub(in crate::app) async fn next(&mut self) -> SqlReadProbeEvent {
        self.events
            .recv()
            .await
            .expect("SQL probe remains captured")
    }
}
