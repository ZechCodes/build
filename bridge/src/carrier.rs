//! The carrier boundary: what the app pushes to a client session, and who owns
//! the session keys.
//!
//! A carrier is a wire, not a protocol. The relay socket is one; a WebRTC
//! DataChannel is another. Everything above this line — the session key, the
//! encrypted frames, dispatch and teardown — is the same on either, so nothing
//! above it learns which wire is carrying.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::transport::{self, Envelope, Frame, OuterFields};

mod dispatch;

use dispatch::Dispatcher;

/// One encrypted frame bound for one client session, before any wire wrapper
/// exists. The carrier that takes it decides how to frame it: the relay writer
/// wraps it as `{"type":"e2ee_envelope",…}`, a DataChannel sends the envelope
/// JSON directly. The session it is bound for is the envelope's own — one fact,
/// stamped once, by `encrypt_frame`.
#[derive(Debug, Clone)]
pub struct OutboundEnvelope(Envelope);

impl OutboundEnvelope {
    pub fn new(envelope: Envelope) -> Self {
        OutboundEnvelope(envelope)
    }

    pub fn session_id(&self) -> &str {
        &self.0.session_id
    }

    pub fn envelope(&self) -> &Envelope {
        &self.0
    }
}

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl SessionSender {
    /// The session this sender targets.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn keyed(
        session_id: &str,
        session_key: String,
        out: mpsc::UnboundedSender<OutboundEnvelope>,
    ) -> Self {
        SessionSender {
            session_id: session_id.to_string(),
            session_key,
            out,
        }
    }

    /// A sender not bound to a live connection — for tests and request/response
    /// callers that never push. `push` succeeds-into-the-void.
    pub fn detached(session_id: impl Into<String>) -> Self {
        let (out, _rx) = mpsc::unbounded_channel();
        SessionSender {
            session_id: session_id.into(),
            session_key: String::new(),
            out,
        }
    }

    /// Test-only: a sender with a real session key and a captured channel, so
    /// tests can decrypt every pushed frame (`term.output`, `term.closed`, …)
    /// with [`decrypt_push`](Self::decrypt_push).
    #[cfg(test)]
    pub fn observable(
        session_id: impl Into<String>,
    ) -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>, String) {
        let (out, rx) = mpsc::unbounded_channel();
        let session_key = transport::generate_session_key();
        (
            SessionSender {
                session_id: session_id.into(),
                session_key: session_key.clone(),
                out,
            },
            rx,
            session_key,
        )
    }

    /// Test-only: decode one captured [`Self::observable`] envelope back to the
    /// pushed inner payload.
    #[cfg(test)]
    pub fn decrypt_push(session_key: &str, outbound: &OutboundEnvelope) -> Value {
        transport::decrypt_envelope(session_key, outbound.envelope())
            .expect("push decrypts with the session key")
            .payload
    }

    /// Encrypt `payload` as an inner frame and hand it to the carrier. Returns
    /// false once the carrier is gone (so the app can drop the stale sender).
    pub fn push(&self, payload: Value) -> bool {
        let envelope = match transport::encrypt_frame(
            &self.session_key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: transport::session_route(&self.session_id),
            },
            &transport::FrameFields {
                frame_type: "data".into(),
                sender: "device".into(),
                payload,
                message_id: None,
                created_at: None,
            },
            None,
        ) {
            Ok(env) => env,
            Err(_) => return false,
        };
        self.out.send(OutboundEnvelope::new(envelope)).is_ok()
    }
}

/// What went wrong with one frame on one carrier. Never fatal to the carrier:
/// a session the device does not know is a frame it drops, not a wire it closes.
#[derive(Debug, thiserror::Error)]
pub enum CarrierError {
    #[error("no session key for {0}")]
    UnknownSession(String),
    #[error("session {0} is already open under a different key")]
    KeyMismatch(String),
    #[error("transport error: {0}")]
    Transport(#[from] transport::TransportError),
}

/// A carrier, as the registry knows one: process-unique, so "which carriers does
/// this session ride" has an answer that outlives any one of them. Minted only
/// by [`CarrierHandle::open`], and never named outside this module — a caller
/// hands over the wire it holds and the registry reads the id off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CarrierId(u64);

/// One live wire — a relay socket generation, or a DataChannel — as everything
/// above the wire sees it: somewhere to put envelopes for any session, since one
/// wire carries every client session of the device.
#[derive(Clone)]
pub struct CarrierHandle {
    id: CarrierId,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl CarrierHandle {
    /// Open one wire: the handle everything above the wire pushes into, and the
    /// queue the wire's writer drains. Minting both here is what keeps the two
    /// halves of one carrier from ever being crossed with another's.
    pub fn open() -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>) {
        static NEXT_CARRIER_ID: AtomicU64 = AtomicU64::new(1);
        let (out, envelopes) = mpsc::unbounded_channel();
        (
            CarrierHandle {
                id: CarrierId(NEXT_CARRIER_ID.fetch_add(1, Ordering::Relaxed)),
                out,
            },
            envelopes,
        )
    }
}

/// One open session: the key it was minted with and the carriers riding it.
struct OpenSession {
    key: String,
    carriers: HashSet<CarrierId>,
}

/// The one home of `session_id → (session key, carriers riding it)`.
///
/// It is shared and it outlives every carrier, which is what lets a session
/// minted over one relay socket keep working over a DataChannel and over the
/// next relay socket. It owns the teardown rule and is the only place that rule
/// is written: **a session ends when its last carrier is gone, or when its
/// client says so**. A caller gets a decrypted frame and a sender back; the key
/// material never leaves this module.
#[derive(Default)]
struct SessionRegistry {
    sessions: Mutex<HashMap<String, OpenSession>>,
}

impl SessionRegistry {
    /// Mint a session, or attach another carrier to one already open.
    ///
    /// Idempotent for a known session whose key matches — that is the re-attach
    /// a browser performs when its relay socket reconnects. A known session
    /// presented under a different key is refused; the frame is dropped.
    fn open(
        &self,
        session_id: &str,
        session_key: String,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(session_id) {
            Some(open) if open.key != session_key => {
                Err(CarrierError::KeyMismatch(session_id.to_string()))
            }
            Some(open) => {
                open.carriers.insert(carrier.id);
                Ok(())
            }
            None => {
                sessions.insert(
                    session_id.to_string(),
                    OpenSession {
                        key: session_key,
                        carriers: HashSet::from([carrier.id]),
                    },
                );
                Ok(())
            }
        }
    }

    /// Take one envelope off a carrier: decrypt it with the session's key and
    /// hand back the frame with a sender that pushes to this carrier. The ride
    /// is recorded only once the frame decrypts — the envelope is authenticated,
    /// so a frame that does not is the one thing that must never bind a session
    /// to a wire — and recording it is what makes a session reachable from
    /// whichever wire its frames arrive on.
    fn admit(
        &self,
        envelope: &Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(Frame, SessionSender), CarrierError> {
        let session_key = self.key_of(&envelope.session_id)?;
        let frame = transport::decrypt_envelope(&session_key, envelope)?;
        {
            let mut sessions = self.sessions.lock().unwrap();
            let open = sessions
                .get_mut(&envelope.session_id)
                .ok_or_else(|| CarrierError::UnknownSession(envelope.session_id.clone()))?;
            open.carriers.insert(carrier.id);
        }
        let sender = SessionSender::keyed(&envelope.session_id, session_key, carrier.out.clone());
        Ok((frame, sender))
    }

    fn key_of(&self, session_id: &str) -> Result<String, CarrierError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|open| open.key.clone())
            .ok_or_else(|| CarrierError::UnknownSession(session_id.to_string()))
    }

    /// One carrier stops carrying one session.
    fn release_session(&self, session_id: &str, carrier: &CarrierHandle) -> Vec<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(open) = sessions.get_mut(session_id) else {
            return Vec::new();
        };
        open.carriers.remove(&carrier.id);
        if !open.carriers.is_empty() {
            return Vec::new();
        }
        sessions.remove(session_id);
        vec![session_id.to_string()]
    }

    /// One carrier is gone: every session that rode nothing else ends with it.
    fn release_carrier(&self, carrier: &CarrierHandle) -> Vec<String> {
        let mut ended = Vec::new();
        self.sessions.lock().unwrap().retain(|session_id, open| {
            open.carriers.remove(&carrier.id);
            if open.carriers.is_empty() {
                ended.push(session_id.clone());
                return false;
            }
            true
        });
        ended
    }

    /// The client said so: the session ends however many carriers it rides.
    fn end(&self, session_id: &str) -> Vec<String> {
        match self.sessions.lock().unwrap().remove(session_id) {
            Some(_) => vec![session_id.to_string()],
            None => Vec::new(),
        }
    }
}

/// "One envelope arrived on some carrier" — admit it, honour a `close` frame,
/// else dispatch it, and emit the synthetic `close` frame of every session the
/// registry reports ended.
///
/// One of these is built in `main.rs` and shared by every carrier, so the
/// worker pool, the ordered terminal lanes and the read fold are one set of
/// resources however many wires the device is carrying. It reads the session id
/// off the envelope: a carrier binds to no session.
pub struct FrameIntake {
    registry: SessionRegistry,
    dispatcher: Dispatcher,
}

impl FrameIntake {
    pub fn new(handler: FrameHandler) -> Arc<Self> {
        Arc::new(FrameIntake {
            registry: SessionRegistry::default(),
            dispatcher: Dispatcher::new(handler),
        })
    }

    /// A client opened a session on this carrier (or re-attached an open one).
    pub fn open(
        &self,
        session_id: &str,
        session_key: String,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        self.registry.open(session_id, session_key, carrier)
    }

    /// One envelope arrived on this carrier: admit it through the registry,
    /// honour a `close` frame, else dispatch it. A frame for a session the
    /// device does not know is refused, and the carrier carries on.
    pub async fn accept(
        &self,
        envelope: Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let (frame, sender) = self.registry.admit(&envelope, carrier)?;
        if frame.frame_type == "close" {
            self.close_ended(self.registry.end(&envelope.session_id));
            return Ok(());
        }
        self.dispatcher.dispatch(sender, frame).await;
        Ok(())
    }

    /// This carrier stops carrying this session — the relay's `session_closed`,
    /// or a channel that closed under one session.
    pub fn close_session(&self, session_id: &str, carrier: &CarrierHandle) {
        self.close_ended(self.registry.release_session(session_id, carrier));
    }

    /// This carrier is gone.
    pub fn close_carrier(&self, carrier: &CarrierHandle) {
        self.close_ended(self.registry.release_carrier(carrier));
    }

    fn close_ended(&self, ended: Vec<String>) {
        for session_id in ended {
            self.dispatcher.close_session(&session_id);
        }
    }
}

/// Handles a decrypted request frame. Receives a [`SessionSender`] (so it can
/// register the session for server-initiated pushes) and returns the response
/// payload to send back.
pub type FrameHandler = Arc<dyn Fn(SessionSender, Frame) -> Value + Send + Sync>;

#[cfg(test)]
mod sender_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_push_hands_the_carrier_an_envelope_and_no_wire_wrapper() {
        let (sender, mut pushes, key) = SessionSender::observable("s-1");

        assert!(sender.push(json!({ "id": 7, "ok": true })));

        let outbound = pushes.try_recv().expect("the push arrived");
        assert_eq!(outbound.session_id(), "s-1");
        assert_eq!(outbound.envelope().route_to, "session:s-1");
        assert_eq!(
            SessionSender::decrypt_push(&key, &outbound),
            json!({ "id": 7, "ok": true })
        );
    }

    #[test]
    fn a_push_to_a_gone_carrier_reports_failure() {
        let (sender, pushes, _key) = SessionSender::observable("s-gone");
        drop(pushes);

        assert!(!sender.push(json!({ "ok": true })));
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::transport::FrameFields;
    use serde_json::json;

    pub(super) fn client_envelope(
        session_key: &str,
        session_id: &str,
        frame_type: &str,
    ) -> Envelope {
        transport::encrypt_frame(
            session_key,
            &OuterFields {
                session_id: session_id.to_string(),
                route_to: "device:d-1".into(),
            },
            &FrameFields {
                frame_type: frame_type.into(),
                sender: "client".into(),
                payload: json!({ "id": 1, "method": "board.list" }),
                message_id: None,
                created_at: None,
            },
            None,
        )
        .expect("the client can encrypt to its own session key")
    }

    #[test]
    fn a_session_opened_on_a_carrier_admits_that_carrier_s_frames() {
        let registry = SessionRegistry::default();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();

        registry
            .open("s-1", key.clone(), &carrier)
            .expect("a fresh session opens");

        let (frame, sender) = registry
            .admit(&client_envelope(&key, "s-1", "data"), &carrier)
            .expect("a frame for a known session is admitted");
        assert_eq!(frame.payload["method"], "board.list");
        assert_eq!(sender.session_id(), "s-1");
    }

    #[test]
    fn a_frame_for_an_unknown_session_is_refused() {
        let registry = SessionRegistry::default();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();

        let refused = registry.admit(&client_envelope(&key, "s-unknown", "data"), &carrier);

        assert!(matches!(refused, Err(CarrierError::UnknownSession(id)) if id == "s-unknown"));
    }

    #[test]
    fn a_session_that_ends_is_forgotten_with_its_key() {
        let registry = SessionRegistry::default();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &carrier).unwrap();

        assert_eq!(registry.end("s-1"), vec!["s-1".to_string()]);

        assert!(matches!(
            registry.admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
        assert!(
            registry.end("s-1").is_empty(),
            "a session ends once, however often it is asked to"
        );
    }

    #[test]
    fn a_session_ends_with_the_last_carrier_that_rides_it() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();
        registry.open("s-1", key.clone(), &peer).unwrap();

        assert!(
            registry.release_session("s-1", &relay).is_empty(),
            "one carrier gone is not the session gone"
        );
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .is_ok());

        assert_eq!(
            registry.release_session("s-1", &peer),
            vec!["s-1".to_string()],
            "the last carrier takes the session with it"
        );
    }

    #[test]
    fn a_carrier_gone_ends_only_the_sessions_that_rode_it_alone() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-alone", key.clone(), &relay).unwrap();
        registry.open("s-shared", key.clone(), &relay).unwrap();
        registry.open("s-shared", key.clone(), &peer).unwrap();

        assert_eq!(
            registry.release_carrier(&relay),
            vec!["s-alone".to_string()]
        );
        assert!(registry
            .admit(&client_envelope(&key, "s-shared", "data"), &peer)
            .is_ok());
    }

    #[test]
    fn a_frame_arriving_on_a_new_carrier_records_the_ride() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();

        registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .expect("any carrier may deliver a frame for a known session");

        assert!(
            registry.release_session("s-1", &relay).is_empty(),
            "the session rides the carrier its frame arrived on"
        );
    }

    #[test]
    fn reopening_a_session_with_its_own_key_re_attaches_the_new_carrier() {
        let registry = SessionRegistry::default();
        let (first, _first_out) = CarrierHandle::open();
        let (second, _second_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &first).unwrap();

        registry
            .open("s-1", key.clone(), &second)
            .expect("the same session on a second carrier is a re-attach");

        assert!(registry.release_carrier(&first).is_empty());
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &second)
            .is_ok());
    }

    #[test]
    fn reopening_a_session_under_a_different_key_is_refused() {
        let registry = SessionRegistry::default();
        let (first, _first_out) = CarrierHandle::open();
        let (second, _second_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &first).unwrap();

        let refused = registry.open("s-1", transport::generate_session_key(), &second);

        assert!(matches!(refused, Err(CarrierError::KeyMismatch(id)) if id == "s-1"));
        assert!(
            registry
                .admit(&client_envelope(&key, "s-1", "data"), &first)
                .is_ok(),
            "the session keeps the key it was opened with"
        );
    }

    #[test]
    fn a_frame_that_does_not_decrypt_records_no_ride() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (forged, _forged_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();

        let refused = registry.admit(
            &client_envelope(&transport::generate_session_key(), "s-1", "data"),
            &forged,
        );

        assert!(matches!(refused, Err(CarrierError::Transport(_))));
        assert_eq!(
            registry.release_session("s-1", &relay),
            vec!["s-1".to_string()],
            "a frame that proves no key possession puts the session on no carrier"
        );
    }

    #[test]
    fn a_sender_the_registry_builds_pushes_to_the_admitting_carrier() {
        let registry = SessionRegistry::default();
        let (carrier, mut out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &carrier).unwrap();

        let (_frame, sender) = registry
            .admit(&client_envelope(&key, "s-1", "data"), &carrier)
            .unwrap();
        assert!(sender.push(json!({ "id": 1, "ok": true })));

        let outbound = out.try_recv().expect("the push rode the carrier");
        assert_eq!(
            SessionSender::decrypt_push(&key, &outbound),
            json!({ "id": 1, "ok": true })
        );
    }
}

#[cfg(test)]
mod intake_tests {
    use super::registry_tests::*;
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    /// The intake's effect side: a session that ended gets its synthetic `close`
    /// frame, whoever reported the end.
    fn watching_intake() -> (Arc<FrameIntake>, mpsc::UnboundedReceiver<String>) {
        let (seen, closes) = mpsc::unbounded_channel();
        let handler: FrameHandler = Arc::new(move |sender, frame| {
            let _ = seen.send(format!("{}:{}", frame.frame_type, sender.session_id()));
            json!({ "ok": true })
        });
        (FrameIntake::new(handler), closes)
    }

    async fn next_seen(closes: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(Duration::from_secs(5), closes.recv())
            .await
            .expect("the handler ran in time")
            .expect("the channel is open")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_close_frame_ends_the_session_outright() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake
            .accept(client_envelope(&key, "s-1", "close"), &carrier)
            .await
            .expect("a close frame is accepted");

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
        assert!(matches!(
            intake
                .registry
                .admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_carrier_gone_closes_the_sessions_that_ended_with_it() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake.close_carrier(&carrier);

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_ends_a_session_riding_only_that_carrier() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake.close_session("s-1", &carrier);

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
        assert!(matches!(
            intake
                .registry
                .admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_leaves_a_session_a_second_carrier_still_rides() {
        let (intake, mut seen) = watching_intake();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &relay).unwrap();
        intake.open("s-1", key.clone(), &peer).unwrap();

        intake.close_session("s-1", &relay);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            seen.try_recv().is_err(),
            "the session is still carried, so nothing closed"
        );
        assert!(intake
            .registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frame_for_an_unknown_session_reaches_no_handler() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();

        let refused = intake
            .accept(client_envelope(&key, "s-unknown", "data"), &carrier)
            .await;

        assert!(matches!(refused, Err(CarrierError::UnknownSession(_))));
        assert!(seen.try_recv().is_err(), "nothing was dispatched");
    }
}
