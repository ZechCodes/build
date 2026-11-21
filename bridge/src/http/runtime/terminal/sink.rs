//! Nonblocking enqueue from shared execution and screen producers.
//! The writer owns each reservation until its last fragment is flushed.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::execution::session::{
    DeliveryError, EncodedEvent, EventSink, OutputFence, ReplySink, TerminalInterest,
};

use super::{interests::Interests, wire::Method};

pub(super) const MAX_ORDINARY: usize = 1024 * 1024;
const MAX_SNAPSHOT: usize = 12 * 1024 * 1024;
const MAX_SNAPSHOT_METADATA: usize = 64 * 1024;
const ORDINARY_QUEUE_BYTES: usize = 2 * 1024 * 1024;
const QUEUE_ITEMS: usize = 128;

pub(super) struct SocketSink {
    queue: mpsc::Sender<Outbound>,
    failure: watch::Sender<Option<CloseCode>>,
    ordinary: Arc<Semaphore>,
    // Temporary staging for large envelopes. D2 acquires a shared
    // snapshot reservation before capture and carries it through this sink.
    snapshot: Arc<Semaphore>,
    pub(super) interests: Mutex<Interests>,
}

pub(super) struct Outbound {
    pub(super) id: Option<Arc<str>>,
    pub(super) bytes: Arc<[u8]>,
    pub(super) fence: OutputFence,
    pub(super) reservation: OwnedSemaphorePermit,
    pub(super) attachment: Option<Arc<TerminalInterest>>,
}

impl SocketSink {
    pub(super) fn new() -> (Arc<Self>, mpsc::Receiver<Outbound>) {
        let (queue, receive) = mpsc::channel(QUEUE_ITEMS);
        (
            Arc::new(Self {
                queue,
                failure: watch::channel(None).0,
                ordinary: Arc::new(Semaphore::new(ORDINARY_QUEUE_BYTES)),
                snapshot: Arc::new(Semaphore::new(1)),
                interests: Mutex::default(),
            }),
            receive,
        )
    }

    pub(super) fn reply(self: &Arc<Self>, id: Arc<str>, method: Method) -> Arc<dyn ReplySink> {
        Arc::new(CallSink {
            socket: self.clone(),
            id,
            method,
        })
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

    fn enqueue(
        &self,
        event: EncodedEvent,
        id: Option<Arc<str>>,
        snapshot_data: Option<usize>,
        attachment: Option<Arc<TerminalInterest>>,
    ) -> Result<(), DeliveryError> {
        let size = event.bytes().len();
        let class = payload_class(size, snapshot_data);
        let reservation = match class {
            Some(Class::Ordinary) => self.ordinary.clone().try_acquire_many_owned(size as u32),
            Some(Class::Snapshot) => self.snapshot.clone().try_acquire_owned(),
            None => {
                self.fail(CloseCode::Size);
                return Ok(());
            }
        };
        let Ok(reservation) = reservation else {
            self.fail(CloseCode::Error);
            return Ok(());
        };
        let (bytes, fence) = event.into_wire();
        match self.queue.try_send(Outbound {
            id,
            bytes,
            fence,
            reservation,
            attachment,
        }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.fail(CloseCode::Error);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(DeliveryError::Closed),
        }
    }
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

impl EventSink for SocketSink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        let payload = event.payload();
        let kind = payload["type"].as_str();
        if !matches!(kind, Some("term.output" | "term.reset" | "term.closed")) {
            return Ok(());
        }
        let Some((interest, term_id)) = event.terminal_interest().zip(payload["term_id"].as_str())
        else {
            return Ok(());
        };
        let mut interests = self.interests.lock().unwrap();
        if !interests.observe_push(interest, term_id) {
            return Ok(());
        }
        let snapshot = (kind == Some("term.reset"))
            .then(|| payload["data"].as_str().map(str::len))
            .flatten();
        self.enqueue(event, None, snapshot, None)
    }
}

struct CallSink {
    socket: Arc<SocketSink>,
    id: Arc<str>,
    method: Method,
}

impl ReplySink for CallSink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        let payload = event.payload();
        if payload["id"].as_str() != Some(&self.id) {
            self.socket.fail(CloseCode::Error);
            return Err(DeliveryError::InvalidPayload);
        }
        let attach = self.method.is_attach();
        let attachment = if attach {
            self.finish_attach(payload)?
        } else {
            None
        };
        let snapshot = (attach && payload["ok"] == true)
            .then(|| payload["result"]["snapshot"].as_str().map(str::len))
            .flatten();
        self.socket
            .enqueue(event, Some(self.id.clone()), snapshot, attachment)
    }
}

impl CallSink {
    fn finish_attach(&self, reply: &Value) -> Result<Option<Arc<TerminalInterest>>, DeliveryError> {
        let mut interests = self.socket.interests.lock().unwrap();
        let registration = interests
            .registration(&self.id)
            .ok_or(DeliveryError::Closed)?;
        let term_id = (reply["ok"] == true)
            .then(|| reply["result"]["term_id"].as_str())
            .flatten();
        interests.attach_reply(&self.id, term_id);
        Ok(term_id.map(|_| registration))
    }
}
