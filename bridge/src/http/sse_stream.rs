//! Fetch-stream SSE body with independently fenced output and finite authority.
use std::io;
use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use futures_util::stream;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use super::{
    limits::BufferReservation,
    sse::{DeliveryLease, SseEvent, SseReceiver, SseSender, HEARTBEAT_INTERVAL},
};

type Check = Arc<dyn Fn() -> bool + Send + Sync>;
type Deadline = Arc<dyn Fn() -> Instant + Send + Sync>;
type Close = Arc<dyn Fn() + Send + Sync>;

pub fn application_body(
    sender: SseSender,
    receiver: SseReceiver,
    reservation: Arc<BufferReservation>,
    check: Check,
    deadline: Deadline,
    close: Close,
) -> Body {
    let mut heartbeat =
        tokio::time::interval_at(Instant::now() + HEARTBEAT_INTERVAL, HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let state = StreamState {
        sender,
        receiver,
        reservation,
        check,
        deadline,
        close,
        heartbeat,
        finished: false,
    };
    Body::from_stream(stream::unfold(state, |mut state| async move {
        state.next().await.map(|result| (result, state))
    }))
}

struct StreamState {
    sender: SseSender,
    receiver: SseReceiver,
    reservation: Arc<BufferReservation>,
    check: Check,
    deadline: Deadline,
    close: Close,
    heartbeat: Interval,
    finished: bool,
}

impl StreamState {
    async fn next(&mut self) -> Option<Result<Bytes, io::Error>> {
        if self.finished {
            return None;
        }
        loop {
            if !(self.check)() || Instant::now() >= (self.deadline)() {
                self.finish();
                return Some(Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "HTTP stream authority ended",
                )));
            }
            tokio::select! {
                event = self.receiver.next() => {
                    let Some(lease) = event else { self.finish(); return None; };
                    // Recheck after await before offering any frame to Hyper.
                    if !(self.check)() || Instant::now() >= (self.deadline)() {
                        self.finish();
                        return Some(Err(io::Error::new(io::ErrorKind::PermissionDenied,"HTTP stream authority ended")));
                    }
                    return Some(Ok(Bytes::from_owner(StreamBytes {lease,_reservation:self.reservation.clone()})));
                }
                _ = self.heartbeat.tick() => { let _ = self.sender.try_send(SseEvent::heartbeat()); }
                _ = tokio::time::sleep_until((self.deadline)()) => {}
            }
        }
    }

    fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            self.sender.close();
            (self.close)();
        }
    }
}

impl Drop for StreamState {
    fn drop(&mut self) {
        self.finish();
    }
}
struct StreamBytes {
    lease: DeliveryLease,
    _reservation: Arc<BufferReservation>,
}
impl AsRef<[u8]> for StreamBytes {
    fn as_ref(&self) -> &[u8] {
        self.lease.as_ref()
    }
}
