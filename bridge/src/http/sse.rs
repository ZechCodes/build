//! Transient application SSE output. There is deliberately no event ID/replay log.
//!
//! A dequeued event retains its allocation until its delivery lease is dropped.
//! The socket body keeps that lease until it is polled for more output; bounded
//! HTTP writer buffers and a stalled-write timeout complete the socket bound.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Notify;

pub const MAX_QUEUED_EVENTS: usize = 128;
pub const MAX_QUEUED_BYTES: usize = 1_048_576;
pub const MAX_EVENT_BYTES: usize = 1_048_576;
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
pub const STALLED_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryError {
    Closed,
    Full,
    TooLarge,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent(Arc<str>);

impl SseEvent {
    pub fn json(name: &str, payload: &Value) -> Result<Self, DeliveryError> {
        if !matches!(
            name,
            "hello" | "changes" | "operation" | "event" | "reset_required" | "closing"
        ) {
            return Err(DeliveryError::Unsupported);
        }
        let data = serde_json::to_string(payload).map_err(|_| DeliveryError::Unsupported)?;
        let encoded = format!("event: {name}\ndata: {data}\n\n");
        if encoded.len() > MAX_EVENT_BYTES {
            return Err(DeliveryError::TooLarge);
        }
        Ok(Self(encoded.into()))
    }

    pub fn heartbeat() -> Self {
        Self(": keepalive\n\n".into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Default)]
struct State {
    queue: VecDeque<SseEvent>,
    events: usize,
    bytes: usize,
    closed: bool,
    reset: bool,
}

impl State {
    fn discard_queued(&mut self) {
        for event in self.queue.drain(..) {
            self.events -= 1;
            self.bytes -= event.len();
        }
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Notify,
}

#[derive(Clone)]
pub struct SseSender(Arc<Shared>);

pub struct SseReceiver(Arc<Shared>);

/// Output already offered to the HTTP body, still charged to this binding.
pub struct DeliveryLease {
    event: SseEvent,
    shared: Arc<Shared>,
}

pub fn channel() -> (SseSender, SseReceiver) {
    let shared = Arc::new(Shared::default());
    (SseSender(shared.clone()), SseReceiver(shared))
}

impl SseSender {
    /// Never awaits a socket or waits for consumer capacity.
    pub fn try_send(&self, event: SseEvent) -> Result<(), DeliveryError> {
        let mut state = self.0.state.lock().unwrap();
        if state.closed {
            return Err(DeliveryError::Closed);
        }
        if state.events >= MAX_QUEUED_EVENTS || state.bytes + event.len() > MAX_QUEUED_BYTES {
            state.closed = true;
            state.reset = true;
            state.discard_queued();
            self.0.wake.notify_one();
            return Err(DeliveryError::Full);
        }
        state.bytes += event.len();
        state.events += 1;
        state.queue.push_back(event);
        self.0.wake.notify_one();
        Ok(())
    }

    /// Expiry/revocation closes without emitting any queued application data.
    pub fn close(&self) {
        let mut state = self.0.state.lock().unwrap();
        state.closed = true;
        state.reset = false;
        state.discard_queued();
        self.0.wake.notify_one();
    }

    pub fn is_closed(&self) -> bool {
        self.0.state.lock().unwrap().closed
    }

    pub fn buffered(&self) -> (usize, usize) {
        let state = self.0.state.lock().unwrap();
        (state.events, state.bytes)
    }
}

impl SseReceiver {
    pub async fn next(&mut self) -> Option<DeliveryLease> {
        loop {
            let notified = self.0.wake.notified();
            {
                let mut state = self.0.state.lock().unwrap();
                if let Some(event) = state.queue.pop_front() {
                    return Some(DeliveryLease {
                        event,
                        shared: self.0.clone(),
                    });
                }
                if state.reset && state.events == 0 {
                    let event = reset_event();
                    state.reset = false;
                    state.bytes += event.len();
                    state.events += 1;
                    return Some(DeliveryLease {
                        event,
                        shared: self.0.clone(),
                    });
                }
                if state.closed && !state.reset {
                    return None;
                }
            }
            notified.await;
        }
    }
}

impl Drop for SseReceiver {
    fn drop(&mut self) {
        SseSender(self.0.clone()).close();
    }
}

impl DeliveryLease {
    pub fn as_str(&self) -> &str {
        self.event.as_str()
    }
}

impl Drop for DeliveryLease {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.events -= 1;
        state.bytes -= self.event.len();
        self.shared.wake.notify_one();
    }
}

fn reset_event() -> SseEvent {
    SseEvent::json(
        "reset_required",
        &serde_json::json!({
            "reason": "output_overflow", "recovery": "current_state_refresh", "durable": false
        }),
    )
    .expect("fixed bounded reset event")
}
