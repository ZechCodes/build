//! The carrier boundary: what the app pushes to a client session, and who owns
//! the session keys.
//!
//! A carrier is a wire, not a protocol. The relay socket is one; a WebRTC
//! DataChannel is another. Everything above this line — the session key, the
//! encrypted frames, dispatch and teardown — is the same on either, so nothing
//! above it learns which wire is carrying.
//!
//! **The seam** (strict P2P transport spec, rule 7). A session is minted by a
//! `session_init` arriving on some carrier, and `FrameIntake::open` answers
//! it on that same carrier: the rendezvous is whichever wire asked, and no
//! caller hands the accept anywhere else. That is what a future direct-network
//! mode (LAN, Tailscale) plugs into — a bridge-local listener becomes a second
//! rendezvous with no relay in the process, and needs no new path through this
//! module. `CarrierKind` gains no variant for it, and the one thing in the
//! crate that may ask whether a wire is the relay is rule 1's refusal below:
//! the relay carries the negotiation and never application traffic.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::api::ApiError;
use crate::transport::{self, Envelope, Frame, KeyPairB64, OuterFields, SessionInit};
use crate::transport_ledger::{StderrLedger, TransportEvent, TransportLedger};

mod dispatch;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use dispatch::Dispatcher;

/// One thing bound for a carrier's wire, before any wire wrapper exists: an
/// encrypted frame for a client session, or the `session_accept` answering an
/// init that arrived on that carrier. The carrier that takes it decides how to
/// frame it — the relay writer wraps a frame as `{"type":"e2ee_envelope",…}`, a
/// DataChannel sends the envelope JSON directly, and both send an accept as
/// `session_accept_message`.
///
/// Two variants and no third: everything else a client asks for is a frame,
/// and everything a wire needs to say for itself is that wire's own business
/// (the relay's heartbeat never reaches here).
#[derive(Debug, Clone)]
pub enum OutboundEnvelope {
    /// An encrypted frame. The session it is bound for is the envelope's own —
    /// one fact, stamped once, by `encrypt_frame`.
    Frame(Envelope),
    /// The device's answer to a `session_init`, on the carrier the init arrived
    /// on. The session id rides beside the envelope because the wire shape
    /// names it (and a client reads it before it holds anything to decrypt
    /// with).
    SessionAccept {
        session_id: String,
        envelope: Envelope,
    },
}

impl OutboundEnvelope {
    pub(crate) fn new(envelope: Envelope) -> Self {
        OutboundEnvelope::Frame(envelope)
    }

    pub fn session_id(&self) -> &str {
        match self {
            OutboundEnvelope::Frame(envelope) => &envelope.session_id,
            OutboundEnvelope::SessionAccept { session_id, .. } => session_id,
        }
    }

    /// The envelope either variant carries. Test-only: a writer matches on the
    /// variant, because what it does with the envelope depends on which it is.
    #[cfg(test)]
    pub(crate) fn envelope(&self) -> &Envelope {
        match self {
            OutboundEnvelope::Frame(envelope) => envelope,
            OutboundEnvelope::SessionAccept { envelope, .. } => envelope,
        }
    }
}

/// The JSON an accept goes out as, written once so every carrier sends the
/// same object: the relay puts it on the socket as a text frame, a channel
/// sends it as a message. A client reads one shape whichever wire it minted on.
pub(crate) fn session_accept_message(session_id: &str, envelope: &Envelope) -> Value {
    serde_json::json!({
        "type": "session_accept",
        "session_id": session_id,
        "envelope": envelope,
    })
}

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
    /// Whether the opening this sender was built for is still open — see
    /// [`OpenSession::still_open`].
    still_open: Arc<AtomicBool>,
}

impl SessionSender {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn session_is_open(&self) -> bool {
        self.still_open.load(Ordering::SeqCst)
    }

    /// Test-only: what the registry does to every sender of an opening that
    /// ended.
    #[cfg(test)]
    fn opening_ended(&self) {
        self.still_open.store(false, Ordering::SeqCst);
    }

    fn keyed(
        session_id: &str,
        session_key: String,
        out: mpsc::UnboundedSender<OutboundEnvelope>,
        still_open: Arc<AtomicBool>,
    ) -> Self {
        SessionSender {
            session_id: session_id.to_string(),
            session_key,
            out,
            still_open,
        }
    }

    /// A sender not bound to a live connection — for tests and request/response
    /// callers that never push. It holds no key, so a `push` through it encrypts
    /// against nothing, fails, and reports `false`.
    pub fn detached(session_id: impl Into<String>) -> Self {
        let (out, _rx) = mpsc::unbounded_channel();
        SessionSender::keyed(
            &session_id.into(),
            String::new(),
            out,
            Arc::new(AtomicBool::new(true)),
        )
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
            SessionSender::keyed(
                &session_id.into(),
                session_key.clone(),
                out,
                Arc::new(AtomicBool::new(true)),
            ),
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
                route_to: session_route(&self.session_id),
            },
            &transport::FrameFields {
                frame_type: transport::DATA_FRAME_TYPE.into(),
                sender: transport::SENDER_DEVICE.into(),
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

/// Where a frame the device sends is routed: the client session's address on
/// the relay, which is the relay's own routing convention and the value every
/// device frame carries. A DataChannel ignores it, as the receiving side of the
/// protocol always has (`transport.rs` only checks it is non-empty).
fn session_route(session_id: &str) -> String {
    format!("session:{session_id}")
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

/// What a carrier does with a frame it cannot honour: drop it and carry on,
/// silently but for the one refusal the spec logs (§Shared session registry) —
/// a client contesting a live session someone else holds. Written here because
/// both carriers drop frames, so the next error that must be heard is added
/// once.
pub(crate) fn drop_frame_error(err: &CarrierError) {
    if let CarrierError::KeyMismatch(session_id) = err {
        eprintln!(
            "carrier: session_init for {session_id} refused: the session is open under another key"
        );
    }
}

/// Whether this frame is the negotiation the relay exists for. `rtc.*` and
/// nothing else, prefix-matched on the method the wire spec names: the verb
/// table (`app/rpc.rs`) owns which of them exist, and a signaling verb added
/// there needs nothing added here.
fn is_signaling(frame: &Frame) -> bool {
    frame
        .payload
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| method.starts_with(SIGNALING_PREFIX))
}

/// The method namespace a relay carrier may carry (spec rule 1).
const SIGNALING_PREFIX: &str = "rtc.";

/// The verb that puts an attachment's bytes on the device.
///
/// One verb, named here rather than pattern-matched: what may skip the queue is
/// a decision, and a prefix rule would hand the exemption to the next verb that
/// happens to share a namespace.
const ATTACHMENT_WRITE: &str = "thread.attach";

fn writes_an_attachment(frame: &Frame) -> bool {
    frame.payload.get("method").and_then(Value::as_str) == Some(ATTACHMENT_WRITE)
}

/// "This request is on the device", sent the moment it is admitted.
///
/// A client cannot tell a request that never arrived from one waiting behind
/// eleven agents' work, so it has to assume the worst and give up on its own
/// deadline — which is how a phone reported an attachment as failed while the
/// bridge was busy storing it. The receipt separates the two questions: the
/// deadline before it is about the PATH, and after it the client waits for an
/// answer that is on its way.
///
/// It carries the request's id and nothing else. `ok` is deliberately absent —
/// that field is what says a reply has settled a call, and a receipt settles
/// nothing.
fn receipt_for(frame: &Frame) -> Option<Value> {
    // Only a request is receipted: a push or an answer riding the other way
    // names no method, and nothing is waiting on it.
    frame.payload.get("method").and_then(Value::as_str)?;
    let id = frame.payload.get("id")?.clone();
    if id.is_null() {
        return None;
    }
    Some(json!({ "id": id, "accepted": true }))
}

/// The answer to a `ping`, built here rather than queued for a worker.
///
/// A ping asks one question — "is this path alive" — and the answer is the
/// frame coming back at all. Queued behind the pool it measures the pool
/// instead: under a load of agents a browser's two-second liveness probe timed
/// out on a path that was plainly carrying, the probe closed the terminal
/// channel it had judged, and the session was re-minted every few seconds all
/// day. So the one verb whose whole meaning is the round trip is answered
/// before the queue, off no lock and no state — the shape is `app/rpc.rs`'s
/// [`crate::app::pong`], so the two paths cannot drift.
fn pong_for(frame: &Frame) -> Option<Value> {
    if frame.payload.get("method").and_then(Value::as_str)? != "ping" {
        return None;
    }
    let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
    Some(crate::api::reply(id, Ok(crate::app::pong())))
}

/// The refusal rule 1 names, word for word. `unavailable` from the closed
/// `ApiError` set of `Bridge Wire Protocol Spec.md` — extending that set is a
/// major bump, so the reason rides in `details` instead.
fn not_a_data_plane() -> ApiError {
    ApiError::Unavailable {
        message: "the relay is not a data plane".into(),
        details: Some(serde_json::json!({ "reason": "relay_is_not_a_data_plane" })),
    }
}

/// A carrier, as the registry knows one: process-unique, so "which carriers does
/// this session ride" has an answer that outlives any one of them. Minted only
/// by [`CarrierHandle::open`], and never named outside this module — a caller
/// hands over the wire it holds and the registry reads the id off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CarrierId(u64);

/// Which kind of wire a carrier is. Nothing above the wire is told which wire
/// carries a session — but two things here are: the ledger ("its last channel
/// closed while the rendezvous still carries" is a `channels_lost`), and rule
/// 1's refusal. Nothing else in the crate may match on `Relay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarrierKind {
    Relay,
    Channel,
}

/// One live wire — a relay socket generation, or a DataChannel — as everything
/// above the wire sees it: somewhere to put envelopes for any session, since one
/// wire carries every client session of the device.
///
/// `Clone` is the same wire in another hand, not a second wire: the id and the
/// queue are what identify it, and both are shared.
#[derive(Clone)]
pub(crate) struct CarrierHandle {
    id: CarrierId,
    kind: CarrierKind,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl CarrierHandle {
    /// Open one relay wire: the handle everything above the wire pushes into,
    /// and the queue the wire's writer drains. Minting both here is what keeps
    /// the two halves of one carrier from ever being crossed with another's.
    pub(crate) fn open() -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>) {
        Self::open_as(CarrierKind::Relay)
    }

    /// Open one DataChannel wire.
    pub(crate) fn open_channel() -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>) {
        Self::open_as(CarrierKind::Channel)
    }

    fn open_as(kind: CarrierKind) -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>) {
        static NEXT_CARRIER_ID: AtomicU64 = AtomicU64::new(1);
        let (out, envelopes) = mpsc::unbounded_channel();
        (
            CarrierHandle {
                id: CarrierId(NEXT_CARRIER_ID.fetch_add(1, Ordering::Relaxed)),
                kind,
                out,
            },
            envelopes,
        )
    }
}

/// One open session: the key it was minted with, which opening of its id this
/// is, and the carriers riding it.
struct OpenSession {
    key: String,
    generation: u64,
    carriers: HashMap<CarrierId, CarrierKind>,
    /// Cleared when this opening ends, under the registry lock, and read by
    /// every sender built for it: a frame admitted before the end and run
    /// after it is told apart from the same id's next opening, so its handler
    /// never runs (`dispatch::run_handler`).
    still_open: Arc<AtomicBool>,
}

impl OpenSession {
    /// Whether a DataChannel is among the wires carrying this session.
    fn rides_a_channel(&self) -> bool {
        self.carriers
            .values()
            .any(|kind| *kind == CarrierKind::Channel)
    }

    /// End this opening: every sender built for it is told, so no handler runs
    /// for it from here on, and the end is stamped with this opening's
    /// generation.
    fn end(&self, session_id: &str) -> SessionEnd {
        self.still_open.store(false, Ordering::SeqCst);
        SessionEnd {
            session_id: session_id.to_string(),
            generation: self.generation,
        }
    }
}

/// One opening of a session id that ended. Versioned, because the effects of
/// an end run behind the frames queued ahead of it, and the same id may be
/// opened again before they do — that newer session is not this end's to close.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionEnd {
    session_id: String,
    generation: u64,
}

/// The one home of `session_id → (session key, carriers riding it)`.
///
/// It is shared and it outlives every carrier, which is what lets a session
/// minted over one relay socket keep working over a DataChannel and over the
/// next relay socket. It owns the teardown rule and is the only place that rule
/// is written: **a session ends when its last carrier is gone, or when its
/// client says so**. A caller gets a decrypted frame and a sender back; the key
/// material never leaves this module.
struct SessionRegistry {
    sessions: Mutex<HashMap<String, OpenSession>>,
    minted: AtomicU64,
    /// Where a session's life is written: minted, channels lost, ended. The peer
    /// transport writes `carrying` to the same ledger.
    ledger: Arc<dyn TransportLedger>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::with_ledger(Arc::new(StderrLedger))
    }
}

impl SessionRegistry {
    fn with_ledger(ledger: Arc<dyn TransportLedger>) -> Self {
        SessionRegistry {
            sessions: Mutex::new(HashMap::new()),
            minted: AtomicU64::new(0),
            ledger,
        }
    }

    /// End one opening under the lock and say so, once, on the ledger.
    fn ended(&self, session_id: &str, open: &OpenSession) -> SessionEnd {
        let ended = open.end(session_id);
        self.ledger.record(session_id, TransportEvent::Ended);
        ended
    }

    /// A carrier left a session that still lives: if it was the session's last
    /// channel, the session is carrying nothing, and the ledger says so once.
    fn note_carrier_left(&self, session_id: &str, left: CarrierKind, open: &OpenSession) {
        if left == CarrierKind::Channel && !open.rides_a_channel() {
            self.ledger.record(session_id, TransportEvent::ChannelsLost);
        }
    }

    /// Mint a session, or accept a re-open of one already open.
    ///
    /// Idempotent for a known session whose key matches — that is the re-attach
    /// a browser performs when its relay socket reconnects — and the session
    /// rides the re-attaching carrier from that moment, so the carrier it was
    /// riding before may drop without ending it. A known session presented
    /// under a different key is refused; the frame is dropped.
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
                open.carriers.insert(carrier.id, carrier.kind);
                Ok(())
            }
            None => {
                sessions.insert(
                    session_id.to_string(),
                    OpenSession {
                        key: session_key,
                        generation: self.minted.fetch_add(1, Ordering::Relaxed) + 1,
                        carriers: HashMap::from([(carrier.id, carrier.kind)]),
                        still_open: Arc::new(AtomicBool::new(true)),
                    },
                );
                self.ledger.record(session_id, TransportEvent::Minted);
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
    ///
    /// One lock, held across the decrypt (pure CPU, no await): the key the
    /// frame is checked against and the opening the ride is recorded on are
    /// then the same opening by construction, however the id is ended and
    /// minted again around this call.
    fn admit(
        &self,
        envelope: &Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(Frame, SessionSender), CarrierError> {
        let mut sessions = self.sessions.lock().unwrap();
        let open = sessions
            .get_mut(&envelope.session_id)
            .ok_or_else(|| CarrierError::UnknownSession(envelope.session_id.clone()))?;
        let frame = transport::decrypt_envelope(&open.key, envelope)?;
        if open.carriers.insert(carrier.id, carrier.kind).is_none() {
            crate::rtc::diagnostic(
                &envelope.session_id,
                &format!("carrier_bound kind={:?}", carrier.kind),
            );
        }
        let sender = SessionSender::keyed(
            &envelope.session_id,
            open.key.clone(),
            carrier.out.clone(),
            open.still_open.clone(),
        );
        Ok((frame, sender))
    }

    fn release_session(&self, session_id: &str, carrier: &CarrierHandle) -> Vec<SessionEnd> {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(open) = sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let Some(left) = open.carriers.remove(&carrier.id) else {
            return Vec::new();
        };
        crate::rtc::diagnostic(
            session_id,
            &format!(
                "carrier_released kind={left:?} remaining={}",
                open.carriers.len()
            ),
        );
        if !open.carriers.is_empty() {
            self.note_carrier_left(session_id, left, open);
            return Vec::new();
        }
        let ended = self.ended(session_id, open);
        sessions.remove(session_id);
        vec![ended]
    }

    fn release_carrier(&self, carrier: &CarrierHandle) -> Vec<SessionEnd> {
        let mut ended = Vec::new();
        self.sessions.lock().unwrap().retain(|session_id, open| {
            let Some(left) = open.carriers.remove(&carrier.id) else {
                return true;
            };
            crate::rtc::diagnostic(
                session_id,
                &format!(
                    "carrier_closed kind={left:?} remaining={}",
                    open.carriers.len()
                ),
            );
            if open.carriers.is_empty() {
                ended.push(self.ended(session_id, open));
                return false;
            }
            self.note_carrier_left(session_id, left, open);
            true
        });
        ended
    }

    fn end(&self, session_id: &str) -> Vec<SessionEnd> {
        match self.sessions.lock().unwrap().remove(session_id) {
            Some(open) => vec![self.ended(session_id, &open)],
            None => Vec::new(),
        }
    }

    fn reopened_since(&self, end: &SessionEnd) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .get(&end.session_id)
            .is_some_and(|open| open.generation > end.generation)
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
///
/// Outside the crate it is built and handed to `relay::run`, nothing more; the
/// verbs a carrier drives it with are the crate's own.
pub struct FrameIntake {
    registry: Arc<SessionRegistry>,
    dispatcher: Dispatcher,
    /// The device's durable X25519 keypair clients wrap session keys to. The
    /// intake is its one owner: it opens every `session_init` — the one moment
    /// a session key exists outside the registry — and every carrier that has
    /// to advertise the public half reads it back from here, so the key a
    /// client wraps to is the key the device unwraps with by construction.
    transport: KeyPairB64,
    /// The sessions that have already been told the relay is not a data plane,
    /// so the log says it once per session however long the client keeps
    /// asking. Cleared when the session ends: the next opening of that id is a
    /// different client, and worth hearing about.
    refused: Mutex<HashSet<String>>,
}

impl FrameIntake {
    /// An intake whose ledger is the daemon's stderr.
    pub fn new(handler: FrameHandler, transport: KeyPairB64) -> Arc<Self> {
        Self::with_ledger(handler, transport, Arc::new(StderrLedger))
    }

    /// An intake whose worker pool is as small as the caller says, so a test
    /// can fill it with a handful of frames rather than the several hundred the
    /// daemon's own pool would need. Test builds only — the sizes the daemon
    /// runs are `dispatch.rs`'s.
    #[cfg(any(test, feature = "testing"))]
    pub fn with_pool(
        handler: FrameHandler,
        transport: KeyPairB64,
        queue_depth: usize,
        workers: usize,
    ) -> Arc<Self> {
        Arc::new(FrameIntake {
            registry: Arc::new(SessionRegistry::with_ledger(Arc::new(StderrLedger))),
            dispatcher: Dispatcher::with_capacity(handler, queue_depth, workers),
            transport,
            refused: Mutex::new(HashSet::new()),
        })
    }

    /// An intake writing every session's transport events to `ledger`.
    pub fn with_ledger(
        handler: FrameHandler,
        transport: KeyPairB64,
        ledger: Arc<dyn TransportLedger>,
    ) -> Arc<Self> {
        Arc::new(FrameIntake {
            registry: Arc::new(SessionRegistry::with_ledger(ledger)),
            dispatcher: Dispatcher::new(handler),
            transport,
            refused: Mutex::new(HashSet::new()),
        })
    }

    /// The ledger this intake's sessions are written to — the peer transport
    /// writes its `carrying` events to the same one.
    pub fn ledger(&self) -> Arc<dyn TransportLedger> {
        self.registry.ledger.clone()
    }

    /// The public half of the device's transport keypair: the key a client wraps
    /// a session key to, which this intake can open. No carrier publishes it —
    /// pairing pinned it at the api, and that is where a browser reads it.
    pub fn transport_public_key(&self) -> &str {
        &self.transport.public_key_b64
    }

    /// A client opened a session on this carrier: unwrap its `session_init`
    /// with the device's transport key, register the session key it carried,
    /// and answer — **on that same carrier** — with the encrypted
    /// `session_accept` that proves the device holds it. The wrapped key comes
    /// in and only the proof goes out, so no carrier ever names key material.
    ///
    /// The accept goes out through the carrier rather than back to the caller
    /// (rule 7): a rendezvous is whatever wire an init arrived on, so a
    /// channel-borne init in a future direct mode is answered with no
    /// relay-specific control path in the process.
    pub(crate) fn open(
        &self,
        session_id: &str,
        init: &SessionInit,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let opened = transport::open_session_init(&self.transport.private_key_b64, init)?;
        self.registry
            .open(session_id, opened.session_key_b64.clone(), carrier)?;
        let envelope = transport::build_session_accept(
            &opened.session_key_b64,
            session_id,
            &session_route(session_id),
            None,
        )?;
        // A carrier already gone takes its accept with it; the session it just
        // opened ends with that carrier by the teardown rule.
        let _ = carrier.out.send(OutboundEnvelope::SessionAccept {
            session_id: session_id.to_string(),
            envelope,
        });
        Ok(())
    }

    /// One envelope arrived on this carrier: admit it through the registry,
    /// honour a `close` frame, refuse what the wire it came in on may not
    /// carry, else dispatch it. A frame for a session the device does not know
    /// is refused, and the carrier carries on.
    pub(crate) async fn accept(
        &self,
        envelope: Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let (frame, sender) = self.registry.admit(&envelope, carrier)?;
        if frame.frame_type == transport::CLOSE_FRAME_TYPE {
            crate::rtc::diagnostic(&envelope.session_id, "client_close_frame");
            self.close_ended(self.registry.end(&envelope.session_id));
            return Ok(());
        }
        if carrier.kind == CarrierKind::Relay && !is_signaling(&frame) {
            self.refuse_as_not_a_data_plane(&sender, &frame);
            return Ok(());
        }
        if let Some(pong) = pong_for(&frame) {
            sender.push(pong);
            return Ok(());
        }
        // The receipt goes out the moment the frame is admitted, before
        // anything decides how long answering will take.
        if let Some(receipt) = receipt_for(&frame) {
            sender.push(receipt);
        }
        if writes_an_attachment(&frame) {
            self.answer_without_queueing(sender, frame);
            return Ok(());
        }
        self.dispatcher.dispatch(sender, frame).await;
        Ok(())
    }

    /// Answer a frame on the spot, off the dispatcher's queues entirely.
    ///
    /// For the frames whose cost is their own and nobody else's: an attachment
    /// is a write of bytes the client already holds, and waiting behind a
    /// stranger's worktree scan is what turned a 15 KB upload into a timeout on
    /// a phone. `spawn_blocking`, because the handler takes the app lock and
    /// writes to disk and this runs on the channel's own reader task — which
    /// has to go on reading, not least so the parts of the NEXT upload arrive.
    fn answer_without_queueing(&self, sender: SessionSender, frame: Frame) {
        let handler = self.dispatcher.handler();
        tokio::task::spawn_blocking(move || {
            let answer = handler.call(sender.clone(), frame);
            sender.push(answer);
        });
    }

    /// Rule 1 of the strict P2P transport spec, enforced here because this is
    /// the one place that holds both the decrypted frame and the wire it
    /// arrived on: **the relay never carries application traffic**. The client
    /// is told, in the closed `ApiError` vocabulary of the wire spec, and the
    /// frame is not dispatched.
    ///
    /// Not fatal to anything. The session keeps working — its next `rtc.offer`
    /// is carried as before — and the answer it gets is the one a browser can
    /// act on: `retryable: false`, so it blocks the device rather than
    /// re-sending.
    fn refuse_as_not_a_data_plane(&self, sender: &SessionSender, frame: &Frame) {
        if self.first_refusal_of(sender.session_id()) {
            eprintln!(
                "carrier: session {} tried to run {} over the relay: the relay is not a data plane",
                sender.session_id(),
                frame
                    .payload
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("a frame with no method")
            );
        }
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        sender.push(not_a_data_plane().into_reply(id));
    }

    /// Whether this session's refusal is the first one — the one worth a log
    /// line. Every refusal is answered; only the first is said out loud.
    fn first_refusal_of(&self, session_id: &str) -> bool {
        self.refused.lock().unwrap().insert(session_id.to_string())
    }

    /// A session ended: whatever it was told about the relay, the next client
    /// on that id is owed the line again.
    fn forget_refusals(&self, session_id: &str) {
        self.refused.lock().unwrap().remove(session_id);
    }

    /// This carrier stops carrying this session — the relay's `session_closed`,
    /// or a channel that closed under one session.
    pub(crate) fn close_session(&self, session_id: &str, carrier: &CarrierHandle) {
        self.close_ended(self.registry.release_session(session_id, carrier));
    }

    pub(crate) fn close_carrier(&self, carrier: &CarrierHandle) {
        self.close_ended(self.registry.release_carrier(carrier));
    }

    fn close_ended(&self, ended: Vec<SessionEnd>) {
        for end in ended {
            self.forget_refusals(&end.session_id);
            let registry = self.registry.clone();
            let session_id = end.session_id.clone();
            self.dispatcher
                .close_session(&session_id, move || !registry.reopened_since(&end));
        }
    }
}

/// Handles a decrypted request frame. Receives a [`SessionSender`] (so it can
/// register the session for server-initiated pushes) and returns the response
/// payload to send back.
pub use dispatch::FrameHandler;

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
    use super::testing::client_request;
    use super::*;
    use crate::transport_ledger::RecordingLedger;
    use serde_json::json;

    pub(super) fn client_envelope(
        session_key: &str,
        session_id: &str,
        frame_type: &str,
    ) -> Envelope {
        client_request(
            session_key,
            session_id,
            frame_type,
            json!({ "id": 1, "method": "board.list" }),
        )
    }

    pub(super) fn ended_ids(ended: Vec<SessionEnd>) -> Vec<String> {
        ended.into_iter().map(|end| end.session_id).collect()
    }

    #[test]
    fn a_session_s_life_is_on_the_ledger_minted_to_ended() {
        let ledger = RecordingLedger::new();
        let key = transport::generate_session_key();
        let registry = SessionRegistry::with_ledger(ledger.clone());
        let (carrier, _out) = CarrierHandle::open();
        registry.open("s-1", key.clone(), &carrier).unwrap();
        // A re-attach is the same opening, not a second mint.
        let (again, _again_out) = CarrierHandle::open();
        registry.open("s-1", key.clone(), &again).unwrap();
        registry.end("s-1");
        assert_eq!(ledger.trail_of("s-1"), vec!["minted", "ended"]);
    }

    /// The rendezvous stays; the session's channels go: the session is carrying
    /// nothing, and that is one `channels_lost` — when the LAST channel goes,
    /// not once per channel. A relay carrier going while a channel carries is
    /// not.
    #[test]
    fn losing_the_last_channel_while_the_relay_carries_is_one_channels_lost() {
        let ledger = RecordingLedger::new();
        let key = transport::generate_session_key();
        let registry = SessionRegistry::with_ledger(ledger.clone());
        let (relay, _relay_out) = CarrierHandle::open();
        let (app, _app_out) = CarrierHandle::open_channel();
        let (term, _term_out) = CarrierHandle::open_channel();
        registry.open("s-1", key.clone(), &relay).unwrap();
        registry.open("s-1", key.clone(), &app).unwrap();
        registry.open("s-1", key.clone(), &term).unwrap();
        assert!(registry.release_session("s-1", &app).is_empty());
        assert!(registry.release_session("s-1", &term).is_empty());
        assert_eq!(ledger.trail_of("s-1"), vec!["minted", "channels_lost"]);

        // The mirror image: the relay drops under a live channel. Not a
        // fallback — the channel is still the better wire.
        let (peer, _peer_out) = CarrierHandle::open_channel();
        registry.open("s-2", key.clone(), &relay).unwrap();
        registry.open("s-2", key.clone(), &peer).unwrap();
        assert!(registry
            .release_carrier(&relay)
            .iter()
            .all(|end| end.session_id != "s-2"));
        assert_eq!(ledger.trail_of("s-2"), vec!["minted"]);
    }

    /// A channel that was the session's last carrier ends the session: that is
    /// an end, not a `channels_lost`, and the ledger says so once.
    #[test]
    fn a_channel_that_carried_last_ends_the_session_not_falls_it_back() {
        let ledger = RecordingLedger::new();
        let key = transport::generate_session_key();
        let registry = SessionRegistry::with_ledger(ledger.clone());
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open_channel();
        registry.open("s-1", key.clone(), &relay).unwrap();
        registry.open("s-1", key.clone(), &peer).unwrap();
        assert!(registry.release_carrier(&relay).is_empty());
        assert_eq!(
            ended_ids(registry.release_carrier(&peer)),
            vec!["s-1".to_string()]
        );
        assert_eq!(ledger.trail_of("s-1"), vec!["minted", "ended"]);
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

        assert_eq!(ended_ids(registry.end("s-1")), vec!["s-1".to_string()]);

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
    fn releasing_a_session_the_registry_does_not_hold_ends_nothing() {
        let registry = SessionRegistry::default();
        let (carrier, _out) = CarrierHandle::open();

        assert!(registry.release_session("s-unknown", &carrier).is_empty());
    }

    #[test]
    fn a_carrier_that_never_rode_a_session_cannot_end_it() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (stranger, _stranger_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();

        assert!(registry.release_session("s-1", &stranger).is_empty());

        assert!(
            registry
                .admit(&client_envelope(&key, "s-1", "data"), &relay)
                .is_ok(),
            "the session still rides the carrier that opened it"
        );
    }

    #[test]
    fn a_session_ends_with_the_last_carrier_that_rides_it() {
        let registry = SessionRegistry::default();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();
        registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .unwrap();

        assert!(
            registry.release_session("s-1", &relay).is_empty(),
            "one carrier gone is not the session gone"
        );

        assert_eq!(
            ended_ids(registry.release_session("s-1", &peer)),
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
        registry
            .admit(&client_envelope(&key, "s-shared", "data"), &peer)
            .unwrap();

        assert_eq!(
            ended_ids(registry.release_carrier(&relay)),
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
        registry
            .admit(&client_envelope(&key, "s-1", "data"), &second)
            .expect("the re-attached session takes frames on the new carrier");

        assert!(registry.release_carrier(&first).is_empty());
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &second)
            .is_ok());
    }

    /// The re-attach the spec asks for: a browser whose relay socket reconnects
    /// re-presents its `session_init`, and the session rides the new carrier
    /// from that moment — before any frame has crossed it. Without the ride,
    /// the carrier it was riding before could drop and end the session the
    /// client just re-attached.
    #[test]
    fn reopening_a_session_puts_it_on_the_re_attaching_carrier() {
        let registry = SessionRegistry::default();
        let (first, _first_out) = CarrierHandle::open();
        let (second, _second_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &first).unwrap();

        registry.open("s-1", key.clone(), &second).unwrap();

        assert!(
            registry.release_carrier(&first).is_empty(),
            "the session rides the carrier it re-attached on"
        );
        assert_eq!(
            ended_ids(registry.release_carrier(&second)),
            vec!["s-1".to_string()],
            "and ends with that last carrier"
        );
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
            ended_ids(registry.release_session("s-1", &relay)),
            vec!["s-1".to_string()],
            "a frame that proves no key possession puts the session on no carrier"
        );
    }

    /// The window a second carrier opens: a frame encrypted under a session's
    /// key arrives after that session ended and its id was minted again under
    /// a fresh key. It decrypts under neither key the registry ever held for
    /// the new opening, and the carrier it arrived on rides nothing.
    #[test]
    fn a_frame_under_an_earlier_opening_s_key_rides_nothing_of_the_reopened_session() {
        let registry = SessionRegistry::default();
        let (first, _first_out) = CarrierHandle::open();
        let (second, _second_out) = CarrierHandle::open();
        let (late, _late_out) = CarrierHandle::open();
        let earlier_key = transport::generate_session_key();
        registry.open("s-1", earlier_key.clone(), &first).unwrap();
        registry.end("s-1");
        registry
            .open("s-1", transport::generate_session_key(), &second)
            .unwrap();

        let refused = registry.admit(&client_envelope(&earlier_key, "s-1", "data"), &late);

        assert!(matches!(refused, Err(CarrierError::Transport(_))));
        assert!(
            registry.release_session("s-1", &late).is_empty(),
            "a frame under the earlier key put the session on no carrier"
        );
        assert_eq!(
            ended_ids(registry.release_session("s-1", &second)),
            vec!["s-1".to_string()],
            "the reopened session rode only the carrier that minted it"
        );
    }

    #[test]
    fn an_id_minted_again_outranks_the_end_of_its_earlier_opening() {
        let registry = SessionRegistry::default();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &carrier).unwrap();
        let earlier = registry.end("s-1").remove(0);
        assert!(!registry.reopened_since(&earlier));

        registry.open("s-1", key, &carrier).unwrap();

        assert!(registry.reopened_since(&earlier));
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
    use super::testing::{client_request, reporting_handler, within_patience};
    use super::*;
    use serde_json::json;
    use std::sync::LazyLock;
    use std::time::Duration;

    /// The device transport keypair every test intake holds, so a test can
    /// wrap a session key to it the way a browser does.
    static TRANSPORT: LazyLock<KeyPairB64> = LazyLock::new(transport::generate_transport_keypair);

    /// The intake's effect side: a session that ended gets its synthetic `close`
    /// frame, whoever reported the end.
    fn watching_intake() -> (Arc<FrameIntake>, mpsc::UnboundedReceiver<String>) {
        let (handler, reports) = reporting_handler();
        (FrameIntake::new(handler, TRANSPORT.clone()), reports)
    }

    /// What a browser sends to open a session against `watching_intake`'s device.
    fn session_init(session_id: &str, session_key: &str) -> SessionInit {
        testing::session_init(session_id, &TRANSPORT.public_key_b64, session_key)
    }

    /// The accept the client verifies comes back on the carrier the init
    /// arrived on, as a `session_accept` the carrier's writer shapes for its
    /// own wire.
    fn accepted(outbound: &OutboundEnvelope) -> (&str, &Envelope) {
        match outbound {
            OutboundEnvelope::SessionAccept {
                session_id,
                envelope,
            } => (session_id, envelope),
            OutboundEnvelope::Frame(_) => panic!("the carrier was handed a frame, not an accept"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opening_a_session_answers_with_an_accept_the_client_can_verify() {
        let (intake, _seen) = watching_intake();
        let (carrier, mut out) = CarrierHandle::open();
        let key = transport::generate_session_key();

        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .expect("a fresh session opens");

        let outbound = out.try_recv().expect("the accept rode the carrier");
        let (session_id, accept) = accepted(&outbound);
        assert_eq!(session_id, "s-1");
        transport::verify_session_accept(&key, accept, "s-1")
            .expect("the accept proves the device unwrapped the key");
        assert_eq!(accept.route_to, "session:s-1");
    }

    /// Rule 7: the accept goes back over the carrier the `session_init` arrived
    /// on, whatever kind of wire that is. A channel-borne init — what a future
    /// direct-network rendezvous mints over — is answered on that channel, with
    /// no relay in the process.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_init_over_a_channel_is_accepted_over_that_channel() {
        let (intake, _seen) = watching_intake();
        let (channel, mut out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();

        intake
            .open("s-direct", &session_init("s-direct", &key), &channel)
            .expect("a fresh session opens over a channel");

        let outbound = out.try_recv().expect("the accept rode the channel");
        let (session_id, accept) = accepted(&outbound);
        assert_eq!(session_id, "s-direct");
        transport::verify_session_accept(&key, accept, "s-direct")
            .expect("the accept proves the device unwrapped the key");
    }

    /// A refused init answers nothing: the session is open under another key,
    /// and the carrier that asked is told by silence (the frame is dropped).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_init_under_another_key_puts_no_accept_on_the_carrier() {
        let (intake, _seen) = watching_intake();
        let (carrier, mut out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();
        out.try_recv().expect("the first accept rode the carrier");

        let refused = intake.open(
            "s-1",
            &session_init("s-1", &transport::generate_session_key()),
            &carrier,
        );

        assert!(matches!(refused, Err(CarrierError::KeyMismatch(id)) if id == "s-1"));
        assert!(out.try_recv().is_err(), "nothing was accepted");
    }

    /// A `session_init` wrapped to some other device's key unwraps to nothing
    /// here: no session opens, and no frame for it is admitted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_init_wrapped_to_another_device_opens_nothing() {
        let (intake, _seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        let for_another_device = SessionInit {
            session_id: "s-1".into(),
            device_id: "d-1".into(),
            wrapped_session_key: transport::wrap_session_key(
                &transport::generate_transport_keypair().public_key_b64,
                &key,
            )
            .unwrap(),
        };

        let refused = intake.open("s-1", &for_another_device, &carrier);

        assert!(matches!(refused, Err(CarrierError::Transport(_))));
        assert!(matches!(
            intake
                .registry
                .admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    /// The `close` frame is a frame type, not a method, and it is honoured on
    /// the relay carrier before rule 1 looks at anything: a browser that is
    /// finished with a session says so over the rendezvous it minted it on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_close_frame_ends_the_session_outright() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();

        intake
            .accept(
                client_envelope(&key, "s-1", transport::CLOSE_FRAME_TYPE),
                &carrier,
            )
            .await
            .expect("a close frame is accepted");

        assert_eq!(within_patience(seen.recv()).await, "close:s-1");
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
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();

        intake.close_carrier(&carrier);

        assert_eq!(within_patience(seen.recv()).await, "close:s-1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_ends_a_session_riding_only_that_carrier() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();

        intake.close_session("s-1", &carrier);

        assert_eq!(within_patience(seen.recv()).await, "close:s-1");
        assert!(matches!(
            intake
                .registry
                .admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_for_a_session_the_device_does_not_know_closes_nothing() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();

        intake.close_session("s-unknown", &carrier);

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(seen.try_recv().is_err(), "there was no session to close");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_leaves_a_session_a_second_carrier_still_rides() {
        let (intake, mut seen) = watching_intake();
        let (relay, _relay_out) = CarrierHandle::open();
        let (peer, _peer_out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &relay)
            .unwrap();
        intake
            .accept(client_envelope(&key, "s-1", "data"), &peer)
            .await
            .unwrap();
        assert_eq!(within_patience(seen.recv()).await, "data:s-1");

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

    /// A session's synthetic `close` runs after the frames queued ahead of it,
    /// which can be arbitrarily long behind a slow terminal handler. The same id
    /// may be opened again meanwhile — the browser re-presenting its session
    /// over its next carrier — and the earlier opening's close is not that
    /// session's to receive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_late_close_from_an_earlier_opening_leaves_the_reopened_session_alone() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let (report, mut frames) = reporting_handler();
        let handler = FrameHandler::new(Arc::clone(&report.clock), move |sender, frame, timer| {
            let holds_its_lane = frame.payload["method"] == "term.input";
            let response = (report.dispatch)(sender, frame, timer);
            if holds_its_lane {
                let _ = release_rx.lock().unwrap().recv();
            }
            response
        });
        let intake = FrameIntake::new(handler, TRANSPORT.clone());
        let (first, _first_out) = CarrierHandle::open_channel();
        let (second, _second_out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &first)
            .unwrap();
        intake
            .accept(
                client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 1, "method": "term.input", "params": { "term_id": "term-1" } }),
                ),
                &first,
            )
            .await
            .unwrap();
        assert_eq!(within_patience(frames.recv()).await, "data:s-1");

        intake.close_carrier(&first);
        intake
            .open("s-1", &session_init("s-1", &key), &second)
            .expect("the id is free to mint again once its session ended");
        release_tx.send(()).unwrap();

        intake
            .accept(client_envelope(&key, "s-1", "data"), &second)
            .await
            .expect("the reopened session takes frames");
        assert_eq!(within_patience(frames.recv()).await, "data:s-1");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            frames.try_recv().is_err(),
            "the earlier opening's close never reached the session that replaced it"
        );
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

    /// The race a second carrier makes real: a frame admitted while the
    /// session was open, dispatched after another carrier's end has already
    /// taken the session's lanes. It must never run — the attach would run
    /// after the close and register a sender into a session that is gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frame_admitted_before_the_end_never_runs_after_it() {
        let (intake, mut seen) = watching_intake();
        let (carrier, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();
        let (attach, sender) = intake
            .registry
            .admit(
                &client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 1, "method": "term.attach", "params": { "term_id": "term-1" } }),
                ),
                &carrier,
            )
            .expect("the frame was admitted while the session was open");

        intake.close_carrier(&carrier);
        assert_eq!(within_patience(seen.recv()).await, "close:s-1");

        intake.dispatcher.dispatch(sender, attach).await;

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            seen.try_recv().is_err(),
            "nothing runs for a session after its close"
        );
    }

    /// A `ping` is answered while every worker is busy and the queue behind
    /// them is full.
    ///
    /// The browser's liveness probe gives the pong three seconds and closes the
    /// channel it was asking about when none comes. Queued behind the pool, the
    /// pong measures the pool: under a load of agents that probe timed out on a
    /// path that was carrying, and the session was re-minted every few seconds
    /// for a day. The question a ping asks is whether the frame gets there and
    /// back, so it is answered before the queue.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_ping_answers_while_every_worker_and_the_queue_are_full() {
        let intake = FrameIntake::with_pool(
            FrameHandler::new(
                crate::timing::FrameClock::new(),
                |_sender, _frame, _timer| {
                    // One worker, held long enough that nothing here is
                    // explained by the pool draining — and bounded, so a failed
                    // assertion cannot leave a thread parked for ever.
                    std::thread::sleep(Duration::from_secs(2));
                    json!({ "ok": true })
                },
            ),
            TRANSPORT.clone(),
            1,
            1,
        );
        // A DataChannel, because the relay may carry nothing but signaling.
        let (carrier, mut out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &carrier)
            .unwrap();
        out.try_recv().expect("the accept rode the carrier");

        // The worker takes the first frame and holds it; the second fills the
        // one queue slot behind it.
        for id in [1, 2] {
            intake
                .accept(
                    client_request(
                        &key,
                        "s-1",
                        "data",
                        json!({ "id": id, "method": "board.list" }),
                    ),
                    &carrier,
                )
                .await
                .expect("the frame was admitted");
        }

        intake
            .accept(
                client_request(&key, "s-1", "data", json!({ "id": 3, "method": "ping" })),
                &carrier,
            )
            .await
            .expect("the ping was admitted");

        let pong = answer_past_receipts(&key, &mut out).await;
        assert_eq!(
            pong["id"],
            json!(3),
            "the pong overtook the wedged pool: {pong:?}"
        );
        assert_eq!(pong["ok"], json!(true), "{pong:?}");
        assert_eq!(pong["result"]["pong"], json!(true), "{pong:?}");
        assert_eq!(pong["result"]["push_events"], json!(true), "{pong:?}");
        assert_eq!(
            pong["result"]["api_version"],
            json!(crate::api::API_VERSION),
            "the fast path answers in the shape `route` does: {pong:?}"
        );
    }

    /// The next thing on the wire that is not a receipt.
    ///
    /// Every admitted request is receipted the moment it is taken, so a test
    /// about an ANSWER reads past them.
    async fn answer_past_receipts(
        key: &str,
        out: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
    ) -> Value {
        loop {
            let seen = SessionSender::decrypt_push(key, &within_patience(out.recv()).await);
            if seen.get("accepted").is_none() {
                return seen;
            }
        }
    }

    /// The terminal's own session pings too — and its session was minted over
    /// the relay, whose socket is gone by the time the probe runs.
    ///
    /// So the answer has to come back on the wire the ping arrived on, not on
    /// the one the session was opened over. A pong written to a closed relay
    /// socket is a pong the browser never sees, which is a probe that always
    /// times out, which is a browser that closes a peer that was carrying.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_terminal_session_is_ponged_on_the_channel_it_pinged_over() {
        let intake = FrameIntake::with_pool(
            FrameHandler::new(
                crate::timing::FrameClock::new(),
                |_sender, _frame, _timer| {
                    std::thread::sleep(Duration::from_secs(2));
                    json!({ "ok": true })
                },
            ),
            TRANSPORT.clone(),
            1,
            1,
        );
        // Minted over the relay, the way a terminal session is.
        let (relay, mut relay_out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-term", &session_init("s-term", &key), &relay)
            .unwrap();
        relay_out.try_recv().expect("the accept rode the relay");

        // It rides the term DataChannel from then on, and the relay socket goes.
        let (term, mut term_out) = CarrierHandle::open_channel();
        for id in [1, 2] {
            intake
                .accept(
                    client_request(
                        &key,
                        "s-term",
                        "data",
                        json!({ "id": id, "method": "term.list" }),
                    ),
                    &term,
                )
                .await
                .expect("the frame was admitted");
        }
        intake.close_carrier(&relay);

        intake
            .accept(
                client_request(&key, "s-term", "data", json!({ "id": 9, "method": "ping" })),
                &term,
            )
            .await
            .expect("the ping was admitted");

        let pong = answer_past_receipts(&key, &mut term_out).await;
        assert_eq!(
            pong["id"],
            json!(9),
            "the pong came back on the term channel: {pong:?}"
        );
        assert_eq!(pong["result"]["pong"], json!(true), "{pong:?}");
        assert!(
            relay_out.try_recv().is_err(),
            "and nothing was written to the relay socket the session was minted over"
        );
    }

    /// A frame too big for one DataChannel message still arrives whole, with
    /// the ping fast path in front of the intake.
    ///
    /// An attachment is one RPC carrying the file in its params, so the wire
    /// splits it into parts (`rtc::chunk`) and the channel reader reassembles
    /// them before the intake is handed anything. That ordering is what keeps
    /// the fast path out of a chunked upload's way — it reads a method off a
    /// decrypted frame, and a part is not a frame — and this is the test that
    /// says so, because the alternative is an upload that never answers and a
    /// caller that times out with nothing to blame.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_chunked_upload_is_reassembled_and_answered_past_the_fast_path() {
        let (intake, mut seen) = watching_intake();
        let (channel, mut out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-upload", &session_init("s-upload", &key), &channel)
            .unwrap();
        out.try_recv().expect("the accept rode the carrier");

        // Half a megabyte of base64, the way a screenshot arrives.
        let content = "a".repeat(512 * 1024);
        let envelope = client_request(
            &key,
            "s-upload",
            "data",
            json!({ "id": 7, "method": "thread.attach", "params": { "content_b64": content } }),
        );
        let json = serde_json::to_string(&envelope).expect("an envelope serializes");
        let parts = crate::rtc::chunk::split(&json);
        assert!(
            parts.len() > 1,
            "this payload has to be chunked to be the test it is"
        );

        // The channel reader's own loop: parts in, one whole envelope out, and
        // only then the intake.
        let mut reassembler = crate::rtc::chunk::Reassembler::default();
        let mut delivered = 0;
        for part in &parts {
            if let Some(whole) = reassembler.accept(part).expect("the parts add up") {
                let envelope: Envelope = serde_json::from_str(&whole).expect("a whole envelope");
                intake
                    .accept(envelope, &channel)
                    .await
                    .expect("the reassembled frame was admitted");
                delivered += 1;
            }
        }

        assert_eq!(delivered, 1, "{} parts made one frame", parts.len());
        assert_eq!(
            within_patience(seen.recv()).await,
            "data:s-upload",
            "the upload reached a handler rather than being eaten by the fast path"
        );
    }

    /// Every admitted request is receipted at once, whatever the pool is doing.
    ///
    /// The receipt is the device saying "I have this". It is what lets a client
    /// stop counting the seconds against a queue it cannot see: the deadline
    /// before it is about the wire, and after it the client is waiting for work
    /// it knows was taken.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_admitted_request_is_receipted_while_the_pool_is_wedged() {
        let intake = FrameIntake::with_pool(
            FrameHandler::new(
                crate::timing::FrameClock::new(),
                |_sender, _frame, _timer| {
                    // Long enough that nothing here is explained by the pool having
                    // drained, bounded so the runtime can still shut down.
                    std::thread::sleep(Duration::from_secs(2));
                    json!({ "ok": true })
                },
            ),
            TRANSPORT.clone(),
            1,
            1,
        );
        let (channel, mut out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &channel)
            .unwrap();
        out.try_recv().expect("the accept rode the carrier");

        // One frame takes the worker, one fills the queue, and the third finds
        // `dispatch` waiting for a slot. Each rides its own task, because the
        // receipt goes out BEFORE that wait, which is the whole point.
        for id in [1, 2, 3] {
            let frame = client_request(
                &key,
                "s-1",
                "data",
                json!({ "id": id, "method": "board.list" }),
            );
            let admitted = Arc::clone(&intake);
            let carrier = channel.clone();
            tokio::spawn(async move {
                let _ = admitted.accept(frame, &carrier).await;
            });
        }

        let mut receipted = Vec::new();
        while receipted.len() < 3 {
            let seen = SessionSender::decrypt_push(&key, &within_patience(out.recv()).await);
            assert_eq!(
                seen["accepted"],
                json!(true),
                "a receipt settles nothing: {seen:?}"
            );
            assert!(seen.get("ok").is_none(), "and carries no verdict: {seen:?}");
            receipted.push(seen["id"].as_u64().expect("a receipt names its request"));
        }
        receipted.sort_unstable();
        assert_eq!(receipted, vec![1, 2, 3]);
    }

    /// An attachment is written where it is admitted, not behind the queue.
    ///
    /// The bytes are the client's own and the work is its own: waiting behind a
    /// stranger's worktree scan is what turned a 15 KB upload into a timeout on
    /// a phone. It answers on the carrier it arrived on, with the pool full.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_attachment_is_written_while_the_pool_is_wedged() {
        let intake = FrameIntake::with_pool(
            FrameHandler::new(
                crate::timing::FrameClock::new(),
                |_sender, frame, _timer| {
                    let method = frame.payload["method"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    if method != "thread.attach" {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                    json!({ "id": frame.payload["id"], "ok": true, "result": { "ran": method } })
                },
            ),
            TRANSPORT.clone(),
            1,
            1,
        );
        let (channel, mut out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &channel)
            .unwrap();
        out.try_recv().expect("the accept rode the carrier");

        for id in [1, 2] {
            let frame = client_request(
                &key,
                "s-1",
                "data",
                json!({ "id": id, "method": "board.list" }),
            );
            let admitted = Arc::clone(&intake);
            let carrier = channel.clone();
            tokio::spawn(async move {
                let _ = admitted.accept(frame, &carrier).await;
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await; // the pool is taken
        intake
            .accept(
                client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 3, "method": "thread.attach", "params": { "content_b64": "aGk=" } }),
                ),
                &channel,
            )
            .await
            .expect("the attachment was admitted");

        // Past the three receipts, the first answer on the wire is the
        // attachment's — the two frames ahead of it are still in the pool.
        let mut answers = Vec::new();
        while answers.is_empty() {
            let seen = SessionSender::decrypt_push(&key, &within_patience(out.recv()).await);
            if seen.get("accepted").is_some() {
                continue;
            }
            answers.push(seen);
        }
        assert_eq!(
            answers[0]["result"]["ran"],
            json!("thread.attach"),
            "{answers:?}"
        );
    }

    /// Rule 1: the relay carries the negotiation and nothing else. An app verb
    /// arriving on a relay carrier is refused with the wire spec's closed
    /// `ApiError` shape and never reaches a handler.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_app_verb_on_the_relay_carrier_is_refused_and_never_dispatched() {
        let (intake, mut seen) = watching_intake();
        let (relay, mut out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &relay)
            .unwrap();
        out.try_recv().expect("the accept rode the carrier");

        intake
            .accept(
                client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 7, "method": "session.hello", "params": {} }),
                ),
                &relay,
            )
            .await
            .expect("the frame is answered, not fatal to the carrier");

        let refusal = SessionSender::decrypt_push(
            &key,
            &out.try_recv().expect("the refusal rode the carrier back"),
        );
        assert_eq!(
            refusal,
            json!({
                "id": 7,
                "ok": false,
                "error": "the relay is not a data plane",
                "error_code": "unavailable",
                "retryable": false,
                "details": { "reason": "relay_is_not_a_data_plane" },
            })
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(seen.try_recv().is_err(), "nothing was dispatched");
    }

    /// The other side of rule 1: `rtc.*` is what the relay is for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signaling_on_the_relay_carrier_is_dispatched() {
        let (intake, mut seen) = watching_intake();
        let (relay, _out) = CarrierHandle::open();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &relay)
            .unwrap();

        intake
            .accept(
                client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 1, "method": "rtc.offer", "params": {} }),
                ),
                &relay,
            )
            .await
            .unwrap();

        assert_eq!(within_patience(seen.recv()).await, "data:s-1");
    }

    /// A channel is the data plane: every verb rides it, signaling included.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_app_verb_on_a_channel_is_dispatched() {
        let (intake, mut seen) = watching_intake();
        let (channel, _out) = CarrierHandle::open_channel();
        let key = transport::generate_session_key();
        intake
            .open("s-1", &session_init("s-1", &key), &channel)
            .unwrap();

        intake
            .accept(
                client_request(
                    &key,
                    "s-1",
                    "data",
                    json!({ "id": 1, "method": "session.hello", "params": {} }),
                ),
                &channel,
            )
            .await
            .unwrap();

        assert_eq!(within_patience(seen.recv()).await, "data:s-1");
    }

    /// The log is one line per session, not one per frame: a client that keeps
    /// sending app traffic over the relay is answered every time and said out
    /// loud once. A session that ended and was minted again is a new client,
    /// and is worth saying again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_is_worth_logging_once_per_session() {
        let (handler, _seen) = reporting_handler();
        let intake = FrameIntake::new(handler, TRANSPORT.clone());

        assert!(intake.first_refusal_of("s-1"));
        assert!(!intake.first_refusal_of("s-1"));
        assert!(intake.first_refusal_of("s-2"));

        intake.forget_refusals("s-1");

        assert!(intake.first_refusal_of("s-1"));
        assert!(!intake.first_refusal_of("s-2"));
    }
}
