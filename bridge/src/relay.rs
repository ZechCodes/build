//! The relay client — the bridge's rendezvous with a browser.
//!
//! The device connects to the relay over a WebSocket, authenticates with an
//! Ed25519-signed challenge, and then carries the negotiation of end-to-end
//! encrypted sessions with browser clients. The relay only ever sees the opaque
//! outer envelope (`{version, session_id, route_to, nonce, ciphertext}`) — it
//! routes by `session_id` and forwards JSON it cannot read.
//!
//! It is a rendezvous, not a connection: the device's transport public key is
//! pinned at the api by pairing and read from there, never uploaded here, and
//! the device's liveness is a heartbeat posted to the api (`presence.rs`), not
//! anything this socket reports. This socket does decide *whether* that beat
//! goes out, though — it is the way in, so while it is down there is nothing
//! for the api to call online (`reachability.rs`).
//!
//! Frame flow, per the relay's `/ws/device` protocol:
//! - relay → `{"type":"authenticated","heartbeat_interval_s":N}`
//! - device → `{"type":"heartbeat"}` every N seconds
//! - relay → `{"type":"session_init","session_id":S,"session_init":{...}}`
//! - device → `{"type":"session_accept","session_id":S,"envelope":{...}}`
//! - both → `{"type":"e2ee_envelope","session_id":S,"envelope":{...}}`

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::Message;

use crate::carrier::{self, CarrierError, CarrierHandle, FrameIntake, OutboundEnvelope};
use crate::logline::{say, Throttle};
use crate::reachability::Reachability;
use crate::transport::{self, Envelope, SessionInit};

/// Install the process-level rustls crypto provider, once, before anything
/// opens TLS.
///
/// Two providers are unified into this build — reqwest brings aws-lc-rs,
/// webrtc's `rtc` brings ring — so rustls 0.23 cannot pick one on its own and
/// the first `wss://` connect panics instead. aws-lc-rs is the one reqwest
/// already runs on, so it is the one installed. A second install (a test, a
/// second entry point) is refused by rustls and ignored here: the provider is
/// in place either way.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// The signed-challenge path the relay expects (`{ts}.GET./ws/device`).
const AUTH_PATH: &str = "/ws/device";

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("websocket error: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),
    #[error("transport error: {0}")]
    Transport(#[from] transport::TransportError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("relay silent for {}s", .0.as_secs())]
    Silent(Duration),
    #[error("a frame waited {}s to be handed over; this socket is wedged", .0.as_secs())]
    Wedged(Duration),
    #[error("carrier error: {0}")]
    Carrier(#[from] CarrierError),
}

const DEFAULT_HEARTBEAT_INTERVAL_S: u64 = 30;
const MISSED_HEARTBEATS_BEFORE_SILENT: u32 = 3;

pub fn silence_deadline(heartbeat_interval_s: u64) -> Duration {
    Duration::from_secs(heartbeat_interval_s.max(1)) * MISSED_HEARTBEATS_BEFORE_SILENT
}

/// How long the read loop may spend handing one frame over before this socket
/// is given up as wedged.
///
/// The loop reads and hands over, but the hand-over is backpressured: the
/// dispatcher's queues are bounded, and a full queue makes the caller wait
/// (`carrier/dispatch.rs`). Waiting there is waiting *inside* the read half, so
/// no pong goes out while it lasts and the relay severs the device for silence
/// — on 2026-09-19 at 15:00:57Z it logged exactly that against a user's
/// workstation, mid-workflow with a dozen agents on it, and refused the next
/// three sessions a browser asked for. The sever does not end the wait either:
/// this loop is parked in `accept`, not in the read, so it learns nothing until
/// the pool drains, however long that takes, and until it returns the reconnect
/// loop in `main.rs` cannot run.
///
/// One heartbeat interval, so the socket is given up and redialled a clear
/// margin inside the relay's own `MISSED_HEARTBEATS_BEFORE_SILENT` window
/// rather than after the relay has already written the device off.
pub fn handoff_deadline(heartbeat_interval_s: u64) -> Duration {
    Duration::from_secs(heartbeat_interval_s.max(1))
}

/// A heartbeat round trip slower than this is worth a line: the relay severs
/// the device after three unanswered beats, so one that took a second is a
/// third of the way to the phone reading "Device not reachable".
const SLOW_HEARTBEAT: Duration = Duration::from_secs(1);

/// How often a repeating heartbeat complaint is written. One line a minute
/// says the socket is struggling; sixty would say nothing more.
const HEARTBEAT_LINES: Duration = Duration::from_secs(60);

/// What the heartbeat task and the read loop know together about the relay's
/// answers: when the last ping went out, and whether its pong came back.
///
/// The relay pings the device and severs it for a missed pong; the device
/// pings the relay for the same reason in the other direction. This watch is
/// the device's view of its own pings, which is the earliest sign the socket,
/// or the runtime under it, has stopped keeping up.
struct HeartbeatWatch {
    outstanding: Mutex<Option<Instant>>,
    lines: Throttle,
}

impl HeartbeatWatch {
    fn new() -> Arc<HeartbeatWatch> {
        Arc::new(HeartbeatWatch {
            outstanding: Mutex::new(None),
            lines: Throttle::new(HEARTBEAT_LINES),
        })
    }

    /// A ping is going out now. The line to write, if the previous one was
    /// never answered.
    fn pinged(&self, now: Instant) -> Option<String> {
        let unanswered = self.outstanding.lock().unwrap().replace(now);
        let since = unanswered?;
        let suppressed = self.lines.admit("unanswered")?;
        Some(format!(
            "relay: the heartbeat ping sent {}s ago was never answered; pinging again{}",
            now.duration_since(since).as_secs(),
            crate::logline::suppressed_suffix(suppressed, HEARTBEAT_LINES)
        ))
    }

    /// A pong came in. The line to write, if the round trip was slow.
    fn answered(&self, now: Instant) -> Option<String> {
        let sent = self.outstanding.lock().unwrap().take()?;
        let round_trip = now.duration_since(sent);
        if round_trip < SLOW_HEARTBEAT {
            return None;
        }
        let suppressed = self.lines.admit("slow")?;
        Some(format!(
            "relay: heartbeat round trip took {} ms{}",
            round_trip.as_millis(),
            crate::logline::suppressed_suffix(suppressed, HEARTBEAT_LINES)
        ))
    }
}

impl RelayError {
    /// Whether the relay's name did not resolve: the resolver not answering
    /// yet after a network change or a wake from sleep ("Temporary failure in
    /// name resolution", 34 of them in one evening on 2026-09-24). Nothing
    /// about the relay was learned, so there is no reason to wait out a
    /// backoff meant for a relay that refused or dropped us.
    pub fn is_name_resolution(&self) -> bool {
        let RelayError::Ws(error) = self else {
            return false;
        };
        matches!(
            error.as_ref(),
            tokio_tungstenite::tungstenite::Error::Io(io)
                if io.to_string().contains("failed to lookup address information")
        )
    }
}

/// How soon a relay whose name did not resolve is dialed again.
pub const NAME_RESOLUTION_RETRY: Duration = Duration::from_secs(2);

/// How long to wait before redialing after a socket ended with `outcome`.
///
/// A name that did not resolve is tried again on a short, fixed timer, and
/// neither waits out the backoff nor grows it (#131); anything else waits the
/// backoff's current delay and doubles it for the next time.
pub fn redial_wait(
    outcome: &Result<(), RelayError>,
    backoff: &mut crate::backoff::Backoff,
) -> Duration {
    if outcome.as_ref().is_err_and(RelayError::is_name_resolution) {
        return NAME_RESOLUTION_RETRY;
    }
    let wait = backoff.current();
    backoff.increase();
    wait
}

/// A resolver that stays down is redialed every [`NAME_RESOLUTION_RETRY`];
/// its line is said once a minute, with how many it stands for.
static NAME_RESOLUTION_LINES: Throttle = Throttle::new(Duration::from_secs(60));

/// The line a redial says: why the socket ended and when it is dialed again.
pub fn say_redial(outcome: &Result<(), RelayError>, wait: Duration) {
    let reconnecting = format!("reconnecting in {}s", wait.as_secs());
    match outcome {
        Ok(()) => say(format!("relay disconnected; {reconnecting}")),
        Err(error) if error.is_name_resolution() => {
            if let Some(suppressed) = NAME_RESOLUTION_LINES.admit("name resolution") {
                say(format!(
                    "relay error: {error}; {reconnecting}{}",
                    crate::logline::suppressed_suffix(suppressed, Duration::from_secs(60))
                ));
            }
        }
        Err(error) => say(format!("relay error: {error}; {reconnecting}")),
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for RelayError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        // Boxed: tungstenite's error type is large, and a large `Err` variant
        // bloats every `Result` in this module.
        RelayError::Ws(Box::new(err))
    }
}

/// The device's stable identity as the relay's auth checks it: who it is and
/// the seed that signs its challenge. The transport keypair clients wrap
/// session keys to is not here — it belongs to the [`FrameIntake`] that opens
/// them, and this socket reads the public half back from there.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    /// Ed25519 seed (base64) — signs the auth challenge.
    pub identity_private_key_b64: String,
}

/// Build the authenticated WebSocket upgrade request: the relay verifies an
/// Ed25519 signature over `{timestamp}.GET./ws/device`.
fn auth_request(url: &str, identity: &DeviceIdentity) -> Result<Request<()>, RelayError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
        .to_string();
    let challenge = format!("{timestamp}.GET.{AUTH_PATH}");
    let signature =
        transport::sign_message_b64(&identity.identity_private_key_b64, challenge.as_bytes())?;

    let mut request = url.into_client_request().map_err(RelayError::from)?;
    let headers = request.headers_mut();
    headers.insert(
        "X-Device-Id",
        HeaderValue::from_str(&identity.device_id)
            .map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    headers.insert(
        "X-Timestamp",
        HeaderValue::from_str(&timestamp).map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    headers.insert(
        "X-Signature",
        HeaderValue::from_str(&signature).map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    Ok(request)
}

/// Connect to the relay and run the device session until the socket closes.
///
/// Handles both plain-`ws` URLs (local/dev) and `wss://` (production, e.g.
/// `wss://relay.getbuild.ing/ws/device`) — TLS is rustls with bundled webpki
/// roots (the crate's only TLS feature, so the default connector below can never
/// silently pick native-tls).
///
/// This socket is one carrier of the intake's sessions; what its end does to
/// them is `SessionRegistry`'s rule (`carrier.rs`).
pub async fn run(
    url: &str,
    identity: &DeviceIdentity,
    intake: Arc<FrameIntake>,
    reachable: &Reachability,
) -> Result<(), RelayError> {
    run_with_connector(url, identity, intake, None, reachable).await
}

/// [`run`], with an explicit TLS connector. `None` uses the default (rustls +
/// webpki roots for `wss://`, plain TCP for `ws` URLs); tests inject
/// `Connector::Rustls` trusting a self-signed root to exercise real TLS locally.
pub async fn run_with_connector(
    url: &str,
    identity: &DeviceIdentity,
    intake: Arc<FrameIntake>,
    tls_connector: Option<tokio_tungstenite::Connector>,
    reachable: &Reachability,
) -> Result<(), RelayError> {
    let request = auth_request(url, identity)?;
    let dialled_at = Instant::now();
    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, tls_connector)
            .await?;
    say(format!(
        "relay: socket open to {url} after {} ms",
        dialled_at.elapsed().as_millis()
    ));
    let (sink, mut source) = stream.split();

    // Unbounded so terminal output bursts never block the app under a lock.
    let (control_tx, control_rx) = mpsc::unbounded_channel::<Message>();
    let (carrier, envelopes_rx) = CarrierHandle::open();
    let writer = spawn_writer(sink, control_rx, envelopes_rx);
    let mut connection = RelayConnection::new(control_tx.clone(), &intake, carrier, reachable);

    // Handlers run in the intake, not on this task: below, the loop only reads
    // and hands over. Handing over can still block — the intake's queues are
    // bounded — so it is done on a deadline, and a hand-over that outlasts it
    // ends this socket rather than leaving it undrained.
    let outcome: Result<(), RelayError> = async {
        loop {
            let Ok(next) = tokio::time::timeout(connection.deadline, source.next()).await else {
                return Err(RelayError::Silent(connection.deadline));
            };
            let Some(message) = next else { break };
            let text = match message? {
                Message::Text(text) => text,
                Message::Close(frame) => {
                    say(format!(
                        "relay: socket closed by the relay{}",
                        frame
                            .map(|frame| format!(": {} {}", u16::from(frame.code), frame.reason))
                            .unwrap_or_default()
                    ));
                    break;
                }
                Message::Pong(_) => {
                    if let Some(line) = connection.heartbeats.answered(Instant::now()) {
                        say(line);
                    }
                    continue;
                }
                _ => continue,
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            // Bounded, because handing over can block (see `handoff_deadline`):
            // a socket this loop cannot drain is one no browser can reach, and
            // it is worth more redialled than held.
            let handoff = connection.handoff;
            if tokio::time::timeout(handoff, connection.accept(&msg))
                .await
                .is_err()
            {
                return Err(RelayError::Wedged(handoff));
            }
        }
        Ok(())
    }
    .await;

    drop(connection);
    drop(control_tx);
    let _ = writer.await;
    outcome
}

/// One task owns the sink; everything else queues to it. Two queues meet here:
/// the relay's own control messages, already wire-shaped, and the envelopes the
/// app pushes to sessions, which know nothing of this wire.
fn spawn_writer(
    mut sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    mut control_rx: mpsc::UnboundedReceiver<Message>,
    mut envelopes_rx: mpsc::UnboundedReceiver<OutboundEnvelope>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                control = control_rx.recv() => match control {
                    Some(message) => message,
                    None => break,
                },
                outbound = envelopes_rx.recv() => match outbound {
                    Some(outbound) => as_relay_wire_message(&outbound),
                    None => break,
                },
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
    })
}

/// This socket generation as a carrier: what a relay message may act on, and the
/// heartbeat that keeps the relay believing in the device. Dropping it is the
/// socket generation ending, however that happens — the read loop returning,
/// or the future that runs it being dropped mid-session.
struct RelayConnection<'a> {
    control_tx: mpsc::UnboundedSender<Message>,
    intake: &'a FrameIntake,
    carrier: CarrierHandle,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
    /// The device's own pings and their pongs, shared with the heartbeat task.
    heartbeats: Arc<HeartbeatWatch>,
    /// How long the relay may stay silent before this socket is given up on;
    /// the interval `authenticated` carries sets it.
    deadline: Duration,
    /// How long one frame may take to hand over before this socket is given up
    /// on. Same source, same greeting (see [`handoff_deadline`]).
    handoff: Duration,
    /// Raised while this socket is the device's way in, dropped when it ends.
    /// The heartbeat reads it, so presence says "reachable" and not merely
    /// "running" (`reachability.rs`).
    reachable: Reachability,
}

impl<'a> RelayConnection<'a> {
    fn new(
        control_tx: mpsc::UnboundedSender<Message>,
        intake: &'a FrameIntake,
        carrier: CarrierHandle,
        reachable: &Reachability,
    ) -> Self {
        RelayConnection {
            control_tx,
            intake,
            carrier,
            heartbeat: None,
            heartbeats: HeartbeatWatch::new(),
            deadline: silence_deadline(DEFAULT_HEARTBEAT_INTERVAL_S),
            handoff: handoff_deadline(DEFAULT_HEARTBEAT_INTERVAL_S),
            reachable: reachable.clone(),
        }
    }

    async fn accept(&mut self, msg: &Value) {
        match msg.get("type").and_then(Value::as_str).unwrap_or("") {
            "authenticated" => self.authenticated(msg),
            "session_init" => {
                if let Err(e) = self.open_session(msg) {
                    drop_protocol_error(&e);
                }
            }
            "e2ee_envelope" => {
                if let Err(e) = self.take_envelope(msg).await {
                    drop_protocol_error(&e);
                }
            }
            "session_closed" => {
                if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                    crate::rtc::diagnostic(session_id, "relay_session_closed");
                    self.intake.close_session(session_id, &self.carrier);
                }
            }
            // "response"/"error"/unknown: nothing for the device to do here.
            _ => {}
        }
    }

    /// The relay took the signed challenge: start heartbeating at its interval.
    /// Nothing else is said to the relay — this socket keeps the device
    /// findable and carries the sessions a browser mints on it, and that is
    /// all. It is also the moment the device becomes reachable, which is what
    /// the api's presence is about, so the beat to the api starts here.
    fn authenticated(&mut self, msg: &Value) {
        let interval = msg
            .get("heartbeat_interval_s")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_S);
        self.heartbeat = Some(spawn_heartbeat(
            self.control_tx.clone(),
            interval,
            self.heartbeats.clone(),
        ));
        self.deadline = silence_deadline(interval);
        self.handoff = handoff_deadline(interval);
        self.reachable.reached();
        say(format!(
            "relay: authenticated; heartbeat every {interval}s, silent after {}s",
            self.deadline.as_secs()
        ));
    }

    /// A client opened a session: parse its `session_init` off the relay wire
    /// and hand it to the intake. The `session_accept` goes back through this
    /// carrier's own queue, not through this socket's control channel — the
    /// intake answers whichever wire asked (`carrier.rs`, rule 7). The wrapped
    /// key passes through here unopened.
    fn open_session(&self, msg: &Value) -> Result<(), RelayError> {
        let session_id = field_str(msg, "session_id")?;
        let init: SessionInit = serde_json::from_value(
            msg.get("session_init")
                .cloned()
                .ok_or_else(|| RelayError::Protocol("session_init payload missing".into()))?,
        )
        .map_err(|e| RelayError::Protocol(format!("bad session_init: {e}")))?;

        self.intake.open(&session_id, &init, &self.carrier)?;
        Ok(())
    }

    /// A client sent an encrypted frame on this carrier. Everything past parsing
    /// it off the relay wire — the key, the close rule, dispatch — belongs to the
    /// intake, which answers to every carrier alike.
    async fn take_envelope(&self, msg: &Value) -> Result<(), RelayError> {
        let envelope: Envelope = serde_json::from_value(
            msg.get("envelope")
                .cloned()
                .ok_or_else(|| RelayError::Protocol("envelope missing".into()))?,
        )
        .map_err(|e| RelayError::Protocol(format!("bad envelope: {e}")))?;
        self.intake.accept(envelope, &self.carrier).await?;
        Ok(())
    }
}

impl Drop for RelayConnection<'_> {
    /// The socket is gone: the carrier is released (see `SessionRegistry`) and
    /// the heartbeat that fed it stops, so its hold on the writer's queue goes
    /// with it. The device is no longer reachable either, so the beat to the
    /// api stops until a socket is back — every way this future can end,
    /// including being dropped mid-session, passes through here.
    fn drop(&mut self) {
        self.reachable.lost();
        self.intake.close_carrier(&self.carrier);
        if let Some(task) = self.heartbeat.take() {
            task.abort();
        }
    }
}

fn spawn_heartbeat(
    control_tx: mpsc::UnboundedSender<Message>,
    interval_s: u64,
    watch: Arc<HeartbeatWatch>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s.max(1)));
        // The first tick fires at once; the ping it sends is the first, with
        // nothing outstanding to complain about.
        ticker.tick().await;
        loop {
            if let Some(line) = watch.pinged(Instant::now()) {
                say(line);
            }
            let heartbeat =
                control_tx.send(Message::Text(json!({"type": "heartbeat"}).to_string()));
            let ping = control_tx.send(Message::Ping(Vec::new()));
            if heartbeat.is_err() || ping.is_err() {
                break;
            }
            ticker.tick().await;
        }
    })
}

fn as_relay_wire_message(outbound: &OutboundEnvelope) -> Message {
    let wire = match outbound {
        OutboundEnvelope::Frame(envelope) => json!({
            "type": "e2ee_envelope",
            "session_id": envelope.session_id,
            "envelope": envelope,
        }),
        OutboundEnvelope::SessionAccept {
            session_id,
            envelope,
        } => carrier::session_accept_message(session_id, envelope),
    };
    Message::Text(wire.to_string())
}

fn field_str(msg: &Value, key: &str) -> Result<String, RelayError> {
    msg.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RelayError::Protocol(format!("{key} missing")))
}

/// A frame the device could not honour. Never fatal to the socket: a malformed
/// relay message is this carrier's own business, and what a frame the carrier
/// boundary refused is worth saying belongs to the module that owns frames.
fn drop_protocol_error(err: &RelayError) {
    if let RelayError::Carrier(refused) = err {
        carrier::drop_frame_error(refused);
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    /// The wire wrapper is the relay carrier's whole contribution: an envelope
    /// goes out as `{"type":"e2ee_envelope",…}` and nothing above the carrier
    /// boundary has to know that.
    #[test]
    fn the_writer_wraps_an_outbound_envelope_for_the_relay_wire() {
        let outbound = OutboundEnvelope::new(Envelope {
            version: 1,
            session_id: "s-1".into(),
            route_to: "session:s-1".into(),
            nonce: "bm9uY2U=".into(),
            ciphertext: "Y2lwaGVy".into(),
        });

        let Message::Text(text) = as_relay_wire_message(&outbound) else {
            panic!("the relay carries text frames");
        };
        let wire: Value = serde_json::from_str(&text).expect("the wrapper is JSON");
        assert_eq!(wire["type"], "e2ee_envelope");
        assert_eq!(wire["session_id"], "s-1");
        assert_eq!(
            wire["envelope"],
            serde_json::to_value(outbound.envelope()).unwrap(),
            "the envelope crosses the wire byte-identical"
        );
    }

    /// An accept is a carrier's message too, and the relay's shape for it is
    /// the one the browser has always read off this socket.
    #[test]
    fn the_writer_sends_an_accept_as_the_relay_s_session_accept() {
        let outbound = OutboundEnvelope::SessionAccept {
            session_id: "s-1".into(),
            envelope: Envelope {
                version: 1,
                session_id: "s-1".into(),
                route_to: "session:s-1".into(),
                nonce: "bm9uY2U=".into(),
                ciphertext: "Y2lwaGVy".into(),
            },
        };

        let Message::Text(text) = as_relay_wire_message(&outbound) else {
            panic!("the relay carries text frames");
        };
        let wire: Value = serde_json::from_str(&text).expect("the accept is JSON");
        assert_eq!(wire["type"], "session_accept");
        assert_eq!(wire["session_id"], "s-1");
        assert_eq!(
            wire["envelope"],
            serde_json::to_value(outbound.envelope()).unwrap()
        );
    }
}

#[cfg(test)]
mod heartbeat_watch_tests {
    use super::*;

    #[test]
    fn a_pong_inside_a_second_says_nothing() {
        let watch = HeartbeatWatch::new();
        let sent = Instant::now();
        assert_eq!(
            watch.pinged(sent),
            None,
            "the first ping has nothing to report"
        );
        assert_eq!(watch.answered(sent + Duration::from_millis(200)), None);
    }

    #[test]
    fn a_slow_pong_is_said_once_a_minute() {
        let watch = HeartbeatWatch::new();
        let sent = Instant::now();
        watch.pinged(sent);
        let line = watch
            .answered(sent + Duration::from_millis(2500))
            .expect("a slow line");
        assert_eq!(line, "relay: heartbeat round trip took 2500 ms");
        watch.pinged(sent + Duration::from_secs(30));
        assert_eq!(
            watch.answered(sent + Duration::from_secs(33)),
            None,
            "the second slow pong inside the minute is counted, not said"
        );
    }

    #[test]
    fn a_ping_never_answered_is_said_when_the_next_one_goes_out() {
        let watch = HeartbeatWatch::new();
        let sent = Instant::now();
        assert_eq!(watch.pinged(sent), None);
        let line = watch
            .pinged(sent + Duration::from_secs(30))
            .expect("an unanswered line");
        assert_eq!(
            line,
            "relay: the heartbeat ping sent 30s ago was never answered; pinging again"
        );
        assert_eq!(
            watch.answered(sent + Duration::from_secs(31)),
            Some("relay: heartbeat round trip took 1000 ms".to_string()),
            "the late pong answers the newest ping"
        );
    }
}

#[cfg(test)]
mod silence_tests {
    #[test]
    fn a_relay_is_silent_after_three_missed_heartbeats() {
        assert_eq!(
            super::silence_deadline(30),
            std::time::Duration::from_secs(90)
        );
        assert_eq!(
            super::silence_deadline(0),
            std::time::Duration::from_secs(3)
        );
    }
}

#[cfg(test)]
mod crypto_provider_tests {
    use super::install_crypto_provider;

    /// With two provider features unified into the build (reqwest's aws-lc-rs,
    /// webrtc's ring) rustls has no default until one is installed, and the
    /// first wss:// connect panics. The daemon installs one before anything
    /// opens TLS, and installing again — a test, a second entry point — is a
    /// no-op rather than a failure.
    #[test]
    fn the_daemon_installs_a_process_crypto_provider_and_may_do_so_twice() {
        install_crypto_provider();
        install_crypto_provider();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "no process-level crypto provider: a wss:// connect would panic"
        );
    }
}

#[cfg(test)]
mod redial_tests {
    use super::*;
    use crate::backoff::Backoff;

    /// A relay whose name does not resolve — a real lookup of a name that
    /// cannot exist — is dialed again in two seconds, and the backoff is left
    /// where it was for the failures that deserve it.
    #[tokio::test]
    async fn a_name_that_does_not_resolve_is_redialed_soon_and_grows_nothing() {
        let failed = tokio_tungstenite::connect_async("ws://relay.b131.invalid/ws/device")
            .await
            .map(|_| ())
            .map_err(RelayError::from);
        let mut backoff = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
        backoff.increase();
        backoff.increase();

        assert!(
            failed.as_ref().is_err_and(RelayError::is_name_resolution),
            "{failed:?}"
        );
        assert_eq!(redial_wait(&failed, &mut backoff), NAME_RESOLUTION_RETRY);
        assert_eq!(backoff.current(), Duration::from_secs(8), "untouched");
    }

    /// Anything else waits the backoff out and doubles it.
    #[test]
    fn any_other_end_waits_the_backoff_and_doubles_it() {
        let mut backoff = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
        let refused = Err(RelayError::from(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        )));
        assert!(!refused.as_ref().is_err_and(RelayError::is_name_resolution));
        assert_eq!(redial_wait(&refused, &mut backoff), Duration::from_secs(2));
        assert_eq!(redial_wait(&Ok(()), &mut backoff), Duration::from_secs(4));
        assert_eq!(backoff.current(), Duration::from_secs(8));
    }
}
