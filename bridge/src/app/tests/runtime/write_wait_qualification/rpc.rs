//! Actual encrypted server-side SPA-role RPC adapter for qualification.
use super::client_watch::{ClientWatch, FinalOutcome};
use crate::app::AppState;
use crate::carrier::testing::{ChannelWire, client_request, session_init};
use crate::carrier::{FrameIntake, OutboundEnvelope, SessionSender};
use crate::timing::write_wait::WriteWaitRegistry;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

mod attach;
mod cleanup;
mod receive;
mod tests;

type Waiting = Arc<Mutex<BTreeMap<String, oneshot::Sender<(Value, Instant)>>>>;
type WatchAttachment = Arc<Mutex<Option<ClientWatch>>>;

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
    receive_errors: Arc<Mutex<Vec<String>>>,
    receiver: tokio::task::JoinHandle<()>,
}

pub(super) struct RpcHarness {
    pub state: Arc<Mutex<AppState>>,
    pub registry: Arc<WriteWaitRegistry>,
    intake: Arc<FrameIntake>,
    openings: Vec<Opening>,
    offered: Mutex<BTreeSet<String>>,
    watch: WatchAttachment,
}

#[derive(Debug)]
pub(super) struct RpcFinishReport {
    pub errors: Vec<String>,
    pub pending_at_close: Vec<String>,
    pub pending_after_join: Vec<String>,
    pub joined_receivers: usize,
}

impl RpcFinishReport {
    pub fn is_clean(&self) -> bool {
        self.joined_receivers == 4
            && self.errors.is_empty()
            && self.pending_at_close.is_empty()
            && self.pending_after_join.is_empty()
    }
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
        let watch: WatchAttachment = Arc::new(Mutex::new(None));
        let openings = (0..4)
            .map(|index| Opening::new(index, &intake, Arc::clone(&watch)))
            .collect();
        Self {
            state,
            registry,
            intake,
            openings,
            offered: Mutex::new(BTreeSet::new()),
            watch,
        }
    }

    pub(super) fn attach_client_watch(&mut self, watch: ClientWatch) {
        attach::install(self, watch);
    }

    pub async fn request(
        &self,
        caller: usize,
        original_id: &str,
        method: &str,
        params: Value,
    ) -> ReplyProof {
        let opening = &self.openings[caller];
        let fresh = self.offered.lock().unwrap().insert(original_id.into());
        assert!(
            fresh,
            "an original request is never retried after completion or failure"
        );
        let (sender, receive) = oneshot::channel();
        assert!(
            opening
                .waiting
                .lock()
                .unwrap()
                .insert(original_id.into(), sender)
                .is_none(),
            "an original request is never sent twice"
        );
        let envelope = client_request(
            &opening.key,
            &opening.id,
            "data",
            json!({"id":original_id,"method":method,"params":params}),
        );
        let sent_at = Instant::now();
        if let Some(watch) = self.attached_watch() {
            watch.sent(original_id);
        }
        opening.wire.accept(&self.intake, envelope).await.unwrap();
        let (reply, final_reply_at) = tokio::time::timeout(Duration::from_secs(30), receive)
            .await
            .expect("infrastructure deadline; pending original remains failed evidence")
            .expect("final wire stays live through completion");
        ReplyProof {
            original_id: original_id.into(),
            method: method.into(),
            reply,
            sent_at,
            final_reply_at,
        }
    }

    fn attached_watch(&self) -> Option<ClientWatch> {
        self.watch.lock().unwrap().clone()
    }
}
