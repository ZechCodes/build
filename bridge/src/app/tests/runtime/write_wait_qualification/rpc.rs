//! Actual encrypted server-side SPA-role RPC adapter for qualification.
use crate::app::AppState;
use crate::carrier::testing::{ChannelWire, client_request, session_init};
use crate::carrier::{FrameIntake, OutboundEnvelope, SessionSender};
use crate::timing::write_wait::WriteWaitRegistry;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

mod tests;

type Waiting = Arc<Mutex<BTreeMap<String, oneshot::Sender<Value>>>>;

pub(super) struct ReplyProof {
    pub original_id: String,
    pub method: String,
    pub reply: Value,
    pub sent_at: Instant,
    pub final_reply_at: Instant,
}

struct Opening {
    id: String,
    key: String,
    wire: Arc<ChannelWire>,
    waiting: Waiting,
    offered: Mutex<BTreeSet<String>>,
    receive_errors: Arc<Mutex<Vec<String>>>,
    receiver: tokio::task::JoinHandle<()>,
}

impl Opening {
    fn new(index: usize, intake: &Arc<FrameIntake>) -> Self {
        let id = format!("qual-spa-{index}");
        let key = crate::transport::generate_session_key();
        let mut wire = ChannelWire::open();
        wire.open_session(
            intake,
            &id,
            &session_init(&id, intake.transport_public_key(), &key),
        )
        .unwrap();
        assert!(matches!(
            wire.outbound.try_recv(),
            Ok(OutboundEnvelope::SessionAccept { .. })
        ));
        let (_unused_sender, unused_receiver) = mpsc::unbounded_channel();
        let mut outbound = std::mem::replace(&mut wire.outbound, unused_receiver);
        let waiting: Waiting = Arc::new(Mutex::new(BTreeMap::new()));
        let receive_errors = Arc::new(Mutex::new(Vec::new()));
        let pending = Arc::clone(&waiting);
        let errors = Arc::clone(&receive_errors);
        let receive_key = key.clone();
        let receiver = tokio::spawn(async move {
            while let Some(envelope) = outbound.recv().await {
                let reply = SessionSender::decrypt_push(&receive_key, &envelope);
                // Admission receipts and pushes have their own counters/timers;
                // they cannot settle the final-response oneshot.
                if reply.get("accepted").is_some() || reply.get("type").is_some() {
                    continue;
                }
                let Some(id) = reply["id"].as_str().map(str::to_owned) else {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("final reply has no string id: {reply}"));
                    continue;
                };
                let sender = pending.lock().unwrap().remove(&id);
                match sender {
                    Some(sender) => {
                        if sender.send(reply).is_err() {
                            errors
                                .lock()
                                .unwrap()
                                .push(format!("abandoned original {id}"));
                        }
                    }
                    None => errors
                        .lock()
                        .unwrap()
                        .push(format!("duplicate/unknown original {id}")),
                }
            }
        });
        Self {
            id,
            key,
            wire: Arc::new(wire),
            waiting,
            offered: Mutex::new(BTreeSet::new()),
            receive_errors,
            receiver,
        }
    }
}

pub(super) struct RpcHarness {
    pub state: Arc<Mutex<AppState>>,
    pub registry: Arc<WriteWaitRegistry>,
    intake: Arc<FrameIntake>,
    openings: Vec<Opening>,
}

impl RpcHarness {
    pub fn new(state: Arc<Mutex<AppState>>, registry: Arc<WriteWaitRegistry>) -> Self {
        state
            .lock()
            .unwrap()
            .frame_clock
            .set_write_wait_registry(Arc::clone(&registry));
        let intake = FrameIntake::with_pool(
            AppState::handler(Arc::clone(&state)),
            crate::transport::generate_transport_keypair(),
            128,
            8,
        );
        let openings = (0..4).map(|index| Opening::new(index, &intake)).collect();
        Self {
            state,
            registry,
            intake,
            openings,
        }
    }

    pub(super) fn attach_client_watch(&mut self, _watch: super::client_watch::ClientWatch) {
        // TEST-FIRST seam. The actual encrypted attachment control must fail
        // before receiver/request callbacks drive the supplied watch.
    }

    pub async fn request(
        &self,
        caller: usize,
        original_id: &str,
        method: &str,
        params: Value,
    ) -> ReplyProof {
        let opening = &self.openings[caller];
        assert!(
            opening
                .offered
                .lock()
                .unwrap()
                .insert(original_id.to_string()),
            "an original request is never retried after completion or failure"
        );
        let (sender, receive) = oneshot::channel();
        assert!(
            opening
                .waiting
                .lock()
                .unwrap()
                .insert(original_id.to_string(), sender)
                .is_none(),
            "an original request is never sent twice"
        );
        // The implemented adapter must register an independently watched client
        // pending row here, retained even when no server original exists yet.
        let sent_at = Instant::now();
        opening
            .wire
            .accept(
                &self.intake,
                client_request(
                    &opening.key,
                    &opening.id,
                    "data",
                    json!({"id":original_id,"method":method,"params":params}),
                ),
            )
            .await
            .unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), receive)
            .await
            .expect("infrastructure deadline; pending original remains failed evidence")
            .expect("final wire stays live through completion");
        let final_reply_at = Instant::now();
        assert!(opening.receive_errors.lock().unwrap().is_empty());
        ReplyProof {
            original_id: original_id.into(),
            method: method.into(),
            reply,
            sent_at,
            final_reply_at,
        }
    }

    pub async fn finish(self) {
        // In implementation: stop offered work/producers first, drain every
        // admitted original and mandatory child, verify client/watchdog rows,
        // then explicitly close wire handles and join receiver tasks. A dropped
        // task or oneshot must remain an error, not success-by-disappearance.
        assert!(
            self.openings
                .iter()
                .all(|opening| opening.waiting.lock().unwrap().is_empty())
        );
        for opening in self.openings {
            assert!(opening.receive_errors.lock().unwrap().is_empty());
            let wire = Arc::try_unwrap(opening.wire)
                .ok()
                .expect("all senders joined");
            wire.close(&self.intake);
            tokio::time::timeout(std::time::Duration::from_secs(10), opening.receiver)
                .await
                .expect("closed wire receiver must join")
                .unwrap();
        }
    }
}
