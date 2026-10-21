//! Actual decoded final replies settle exact pending caller identities.
use super::*;

impl Opening {
    pub(super) fn new(index: usize, intake: &Arc<FrameIntake>, watch: WatchAttachment) -> Self {
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
        let outbound = std::mem::replace(&mut wire.outbound, unused_receiver);
        let waiting: Waiting = Arc::new(Mutex::new(BTreeMap::new()));
        let receive_errors = Arc::new(Mutex::new(Vec::new()));
        let receiver = tokio::spawn(receive_finals(
            outbound,
            key.clone(),
            Arc::clone(&waiting),
            Arc::clone(&receive_errors),
            watch,
        ));
        Self {
            id,
            key,
            wire: Arc::new(wire),
            waiting,
            receive_errors,
            receiver,
        }
    }
}

async fn receive_finals(
    mut outbound: mpsc::UnboundedReceiver<OutboundEnvelope>,
    key: String,
    pending: Waiting,
    errors: Arc<Mutex<Vec<String>>>,
    watch: WatchAttachment,
) {
    while let Some(envelope) = outbound.recv().await {
        let _receiving = Receiving::begin(&watch);
        let reply = SessionSender::decrypt_push(&key, &envelope);
        let final_reply_at = Instant::now();
        // Admission receipts and pushes settle neither oneshot nor watch.
        if reply.get("accepted").is_some() || reply.get("type").is_some() {
            continue;
        }
        let Some(id) = reply["id"].as_str().map(str::to_owned) else {
            record_identity_error(
                &watch,
                &errors,
                "<missing-final-id>",
                format!("final reply has no string id: {reply}"),
            );
            continue;
        };
        deliver_final(&pending, &watch, &errors, &id, reply, final_reply_at);
    }
}

fn deliver_final(
    pending: &Waiting,
    watch: &WatchAttachment,
    errors: &Mutex<Vec<String>>,
    id: &str,
    reply: Value,
    final_reply_at: Instant,
) {
    let sender = pending.lock().unwrap().remove(id);
    match sender {
        Some(sender) => {
            let attached = watch.lock().unwrap().active.clone();
            if let Some(watch) = attached {
                let outcome = if reply["ok"] == true {
                    FinalOutcome::Reply
                } else {
                    FinalOutcome::ProtocolError
                };
                watch.decoded_final_at(id, outcome, final_reply_at);
            }
            if sender.send((reply, final_reply_at)).is_err() {
                record_identity_error(watch, errors, id, format!("abandoned original {id}"));
            }
        }
        None => record_identity_error(
            watch,
            errors,
            id,
            format!("duplicate/unknown original {id}"),
        ),
    }
}

fn record_identity_error(
    watch: &WatchAttachment,
    errors: &Mutex<Vec<String>>,
    id: &str,
    error: String,
) {
    errors.lock().unwrap().push(error);
    let retained = watch.lock().unwrap().audit_watch();
    if let Some(watch) = retained {
        watch.record_unmatched_reply(id);
    }
}

/// Counts the actual decoded receive segment, including callback/error delivery.
struct Receiving(WatchAttachment);
impl Receiving {
    fn begin(watch: &WatchAttachment) -> Self {
        watch.lock().unwrap().receiving += 1;
        Self(Arc::clone(watch))
    }
}
impl Drop for Receiving {
    fn drop(&mut self) {
        self.0.lock().unwrap().receiving -= 1;
    }
}
