//! Nonblocking enqueue from shared execution and screen producers.
//! The writer owns each reservation until its last fragment is flushed.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::{
    api::ApiError,
    execution::session::{
        DeliveryError, EncodedEvent, EventSink, OutputFence, ReplySink, TerminalInterest,
        TerminalInterestState, TerminalSnapshotReservation,
    },
};

use super::{interests::Interests, refusal_bytes, wire::Method};

pub(super) const MAX_ORDINARY: usize = 1024 * 1024;
const MAX_SNAPSHOT: usize = 12 * 1024 * 1024;
const MAX_SNAPSHOT_METADATA: usize = 64 * 1024;
const ORDINARY_QUEUE_BYTES: usize = 2 * 1024 * 1024;
const QUEUE_ITEMS: usize = 128;
const CONTROL_BYTES: usize = 8 * 1024;
const CONTROL_ITEMS: usize = 8;

pub(super) struct SocketSink {
    queue: mpsc::Sender<Outbound>,
    failure: watch::Sender<Option<CloseCode>>,
    ordinary: Arc<Semaphore>,
    items: Arc<Semaphore>,
    control: Arc<Semaphore>,
    control_items: Arc<Semaphore>,
    pub(super) interests: Mutex<Interests>,
}

pub(super) struct Outbound {
    pub(super) id: Option<Arc<str>>,
    pub(super) bytes: Arc<[u8]>,
    pub(super) fence: OutputFence,
    pub(super) reservation: Reservation,
    pub(super) delivered: Option<DeliveredCursor>,
    attachment: Option<Arc<TerminalInterest>>,
}

pub(super) struct Reservation {
    _bytes: Option<OwnedSemaphorePermit>,
    _item: Option<OwnedSemaphorePermit>,
    snapshot: Option<Arc<TerminalSnapshotReservation>>,
}

impl Reservation {
    pub(super) fn snapshot_expired(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.is_expired())
    }
}

pub(super) struct DeliveredCursor {
    interest: Arc<TerminalInterest>,
    term_id: String,
    cursor: u64,
}

impl DeliveredCursor {
    fn new(interest: &Arc<TerminalInterest>, value: &Value) -> Option<Self> {
        Some(Self {
            interest: interest.clone(),
            term_id: value["term_id"].as_str()?.into(),
            cursor: value["cursor"].as_u64()?,
        })
    }
}

impl Outbound {
    /// Select the reply as it leaves the queue. After selection, the writer
    /// finishes this message before it can send the replacement's reply.
    pub(super) fn into_write(mut self) -> Option<Self> {
        if let Some(interest) = self.attachment.take() {
            match interest.state() {
                TerminalInterestState::Live => {}
                TerminalInterestState::Detached => return None,
                TerminalInterestState::Superseded => {
                    self.bytes = superseded_refusal(self.id.as_deref().expect("attach reply ID"));
                    self.delivered = None;
                }
            }
        }
        Some(self)
    }
}

fn superseded_refusal(id: &str) -> Arc<[u8]> {
    refusal_bytes(id, ApiError::busy("Terminal attachment was superseded")).into()
}

impl SocketSink {
    pub(super) fn new() -> (Arc<Self>, mpsc::Receiver<Outbound>) {
        let (queue, receive) = mpsc::channel(QUEUE_ITEMS);
        (
            Arc::new(Self {
                queue,
                failure: watch::channel(None).0,
                ordinary: Arc::new(Semaphore::new(ORDINARY_QUEUE_BYTES - CONTROL_BYTES)),
                items: Arc::new(Semaphore::new(QUEUE_ITEMS - CONTROL_ITEMS)),
                control: Arc::new(Semaphore::new(CONTROL_BYTES)),
                control_items: Arc::new(Semaphore::new(CONTROL_ITEMS)),
                interests: Mutex::default(),
            }),
            receive,
        )
    }

    pub(super) fn reply(self: &Arc<Self>, id: Arc<str>, method: Method) -> Arc<CallSink> {
        Arc::new(CallSink {
            socket: self.clone(),
            id,
            method,
            snapshot: Mutex::default(),
        })
    }

    pub(super) fn record_delivery(&self, delivery: DeliveredCursor) {
        self.interests.lock().unwrap().record_delivery(
            &delivery.interest,
            &delivery.term_id,
            delivery.cursor,
        );
    }

    pub(super) async fn failed(&self) -> CloseCode {
        let mut failure = self.failure.subscribe();
        let code = failure
            .wait_for(Option::is_some)
            .await
            .expect("socket owns sender");
        code.expect("checked by wait_for")
    }

    fn fail(&self, code: CloseCode) {
        self.failure.send_if_modified(|failure| {
            if failure.is_some() {
                return false;
            }
            *failure = Some(code);
            true
        });
    }

    fn reserve_output(
        &self,
        size: usize,
        control: bool,
        snapshot: Option<Arc<TerminalSnapshotReservation>>,
    ) -> Option<Reservation> {
        if control {
            if let Some(reservation) = self.reserve_control(size) {
                return Some(reservation);
            }
        }
        let item = self.items.clone().try_acquire_owned().ok()?;
        let bytes = if snapshot.is_some() {
            None
        } else {
            Some(
                self.ordinary
                    .clone()
                    .try_acquire_many_owned(size as u32)
                    .ok()?,
            )
        };
        Some(Reservation {
            _bytes: bytes,
            _item: Some(item),
            snapshot,
        })
    }

    fn reserve_control(&self, size: usize) -> Option<Reservation> {
        let item = self.control_items.clone().try_acquire_owned().ok()?;
        let bytes = self
            .control
            .clone()
            .try_acquire_many_owned(size as u32)
            .ok()?;
        Some(Reservation {
            _bytes: Some(bytes),
            _item: Some(item),
            snapshot: None,
        })
    }

    fn enqueue(&self, output: QueuedMessage) -> Result<(), DeliveryError> {
        if let Some(snapshot) = &output.snapshot {
            snapshot.encoded(output.bytes.len());
        }
        let Some(reservation) =
            self.reserve_output(output.bytes.len(), output.control, output.snapshot)
        else {
            self.fail(CloseCode::Error);
            return Ok(());
        };
        match self.queue.try_send(Outbound {
            id: output.id,
            bytes: output.bytes,
            fence: output.fence,
            reservation,
            attachment: output.attachment,
            delivered: output.delivered,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.fail(CloseCode::Error);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(DeliveryError::Closed),
        }
    }

    fn attachment_error(
        &self,
        interests: &mut Interests,
        interest: &Arc<TerminalInterest>,
        fence: OutputFence,
        error: ApiError,
    ) -> Result<(), DeliveryError> {
        let Some(id) = interests.remove_failed(interest) else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(&json!({
            "type": "attachment_error",
            "attach_id": id,
            "reason": error.code(),
            "bridge_epoch": crate::bridge_epoch::current(),
        }))
        .expect("attachment control serializes");
        self.enqueue(QueuedMessage {
            id: None,
            bytes: bytes.into(),
            fence: fence.terminal_failure(),
            attachment: None,
            delivered: None,
            snapshot: None,
            control: true,
        })
    }
}

struct QueuedMessage {
    id: Option<Arc<str>>,
    bytes: Arc<[u8]>,
    fence: OutputFence,
    attachment: Option<Arc<TerminalInterest>>,
    delivered: Option<DeliveredCursor>,
    snapshot: Option<Arc<TerminalSnapshotReservation>>,
    control: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Class {
    Ordinary,
    Snapshot,
}

fn payload_class(size: usize, snapshot_data: Option<usize>) -> Option<Class> {
    if let Some(data) = snapshot_data {
        if size > MAX_SNAPSHOT || size.saturating_sub(data) > MAX_SNAPSHOT_METADATA {
            return None;
        }
        if size > MAX_ORDINARY {
            return Some(Class::Snapshot);
        }
    }
    (size <= MAX_ORDINARY).then_some(Class::Ordinary)
}

fn snapshot_data_bytes(payload: &Value, reply_to: Option<Method>) -> Option<usize> {
    match reply_to {
        Some(method) if method.is_attach() && payload["ok"] == true => {
            payload["result"]["snapshot"].as_str().map(str::len)
        }
        None if payload["type"] == "term.reset" => payload["data"].as_str().map(str::len),
        _ => None,
    }
}

impl EventSink for SocketSink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        let payload = event.payload();
        let Some(interest) = event.terminal_interest().cloned() else {
            return Ok(());
        };
        let mut interests = self.interests.lock().unwrap();
        if payload["type"] == "attachment_error" {
            let error = if payload["reason"] == ApiError::SnapshotBusy.code() {
                ApiError::SnapshotBusy
            } else {
                ApiError::SnapshotTooLarge
            };
            return self.attachment_error(&mut interests, &interest, event.into_wire().1, error);
        }
        let Some("term.output" | "term.reset" | "term.closed") = payload["type"].as_str() else {
            return Ok(());
        };
        let Some(term_id) = payload["term_id"].as_str() else {
            return Ok(());
        };
        if !interests.observe_push(&interest, term_id) {
            return Ok(());
        }
        self.enqueue_push(event, &interest, &mut interests)
    }
}

impl SocketSink {
    fn enqueue_push(
        &self,
        event: EncodedEvent,
        interest: &Arc<TerminalInterest>,
        interests: &mut Interests,
    ) -> Result<(), DeliveryError> {
        let payload = event.payload();
        let closed = payload["type"] == "term.closed";
        let delivered = (!closed)
            .then(|| DeliveredCursor::new(interest, payload))
            .flatten();
        let data = snapshot_data_bytes(payload, None);
        let (bytes, fence, snapshot) = event.into_wire();
        match payload_class(bytes.len(), data) {
            None if data.is_some() => {
                drop(bytes);
                return self.attachment_error(
                    interests,
                    interest,
                    fence,
                    ApiError::SnapshotTooLarge,
                );
            }
            None => {
                self.fail(CloseCode::Size);
                return Ok(());
            }
            Some(Class::Snapshot) if snapshot.is_none() => {
                self.fail(CloseCode::Error);
                return Ok(());
            }
            _ => {}
        }
        self.enqueue(QueuedMessage {
            id: None,
            bytes,
            fence,
            snapshot,
            delivered,
            attachment: None,
            control: closed,
        })
    }
}

pub(super) struct CallSink {
    socket: Arc<SocketSink>,
    id: Arc<str>,
    method: Method,
    snapshot: Mutex<Option<Arc<TerminalSnapshotReservation>>>,
}

impl ReplySink for CallSink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        if event.payload()["id"].as_str() != Some(&self.id) {
            self.socket.fail(CloseCode::Error);
            return Err(DeliveryError::InvalidPayload);
        }
        if self.method.is_attach() {
            return self.enqueue_attach(event);
        }
        let (bytes, fence, _) = event.into_wire();
        if bytes.len() > MAX_ORDINARY {
            self.socket.fail(CloseCode::Size);
            return Ok(());
        }
        self.socket.enqueue(QueuedMessage {
            id: Some(self.id.clone()),
            bytes,
            fence,
            attachment: None,
            delivered: None,
            snapshot: None,
            control: true,
        })
    }
}

impl CallSink {
    pub(super) fn reserve_snapshot(&self, reservation: Arc<TerminalSnapshotReservation>) {
        *self.snapshot.lock().unwrap() = Some(reservation);
    }

    fn enqueue_attach(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        let mut snapshot = self.snapshot.lock().unwrap().take();
        let payload = event.payload();
        let data = snapshot_data_bytes(payload, Some(self.method));
        let class = payload_class(event.bytes().len(), data);
        let too_large = data.is_some() && class.is_none();
        if class.is_none() && !too_large {
            self.socket.fail(CloseCode::Size);
            return Ok(());
        }
        if class == Some(Class::Snapshot) && snapshot.is_none() {
            self.socket.fail(CloseCode::Error);
            return Ok(());
        }

        let attachment = self.finish_attach(payload, too_large)?;
        let delivered = attachment
            .as_ref()
            .and_then(|interest| DeliveredCursor::new(interest, &payload["result"]));
        let (mut bytes, fence, _) = event.into_wire();
        let control = attachment.is_none();
        if control {
            if too_large {
                bytes = refusal_bytes(&self.id, ApiError::SnapshotTooLarge).into();
            }
            snapshot = None;
        }

        self.socket.enqueue(QueuedMessage {
            id: Some(self.id.clone()),
            bytes,
            fence,
            snapshot,
            attachment,
            delivered,
            control,
        })
    }

    fn finish_attach(
        &self,
        reply: &Value,
        refused: bool,
    ) -> Result<Option<Arc<TerminalInterest>>, DeliveryError> {
        let mut interests = self.socket.interests.lock().unwrap();
        let registration = interests
            .registration(&self.id)
            .ok_or(DeliveryError::Closed)?;
        let term_id = (!refused && reply["ok"] == true)
            .then(|| reply["result"]["term_id"].as_str())
            .flatten();
        interests.attach_reply(&self.id, term_id);
        Ok(term_id.map(|_| registration))
    }
}

#[cfg(test)]
#[path = "sink/tests.rs"]
mod tests;
