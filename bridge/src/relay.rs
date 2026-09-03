//! The relay client — the bridge's link to the world.
//!
//! The device connects to the relay over a WebSocket, authenticates with an
//! Ed25519-signed challenge, uploads its X25519 transport public key, and then
//! relays end-to-end encrypted sessions with browser clients. The relay only ever
//! sees the opaque outer envelope (`{version, session_id, route_to, nonce,
//! ciphertext}`) — it routes by `session_id` and forwards JSON it cannot read.
//!
//! Frame flow, per the relay's `/ws/device` protocol:
//! - relay → `{"type":"authenticated","heartbeat_interval_s":N}`
//! - device → `{"type":"transport_key","transport_public_key":"<b64>"}`
//! - device → `{"type":"heartbeat"}` every N seconds
//! - relay → `{"type":"session_init","session_id":S,"session_init":{...}}`
//! - device → `{"type":"session_accept","session_id":S,"envelope":{...}}`
//! - both → `{"type":"e2ee_envelope","session_id":S,"envelope":{...}}`

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::Message;

use crate::carrier::{CarrierError, CarrierHandle, FrameIntake, OutboundEnvelope};
use crate::transport::{self, Envelope, KeyPairB64, SessionInit};

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
    #[error("carrier error: {0}")]
    Carrier(#[from] CarrierError),
}

const DEFAULT_HEARTBEAT_INTERVAL_S: u64 = 30;
const MISSED_HEARTBEATS_BEFORE_SILENT: u32 = 3;

pub fn silence_deadline(heartbeat_interval_s: u64) -> Duration {
    Duration::from_secs(heartbeat_interval_s.max(1)) * MISSED_HEARTBEATS_BEFORE_SILENT
}

impl From<tokio_tungstenite::tungstenite::Error> for RelayError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        // Boxed: tungstenite's error type is large, and a large `Err` variant
        // bloats every `Result` in this module.
        RelayError::Ws(Box::new(err))
    }
}

/// The device's stable identity: who it is and the keys that prove it.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    /// Ed25519 seed (base64) — signs the auth challenge.
    pub identity_private_key_b64: String,
    /// The durable X25519 transport keypair clients wrap session keys to.
    pub transport: KeyPairB64,
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
/// This socket is one carrier of the intake's sessions: every session minted
/// here is reachable from every other carrier that intake serves, and when the
/// socket ends the carrier is released — the sessions that rode nothing else end
/// with it.
pub async fn run(
    url: &str,
    identity: &DeviceIdentity,
    intake: Arc<FrameIntake>,
) -> Result<(), RelayError> {
    run_with_connector(url, identity, intake, None).await
}

/// [`run`], with an explicit TLS connector. `None` uses the default (rustls +
/// webpki roots for `wss://`, plain TCP for `ws` URLs); tests inject
/// `Connector::Rustls` trusting a self-signed root to exercise real TLS locally.
pub async fn run_with_connector(
    url: &str,
    identity: &DeviceIdentity,
    intake: Arc<FrameIntake>,
    tls_connector: Option<tokio_tungstenite::Connector>,
) -> Result<(), RelayError> {
    let request = auth_request(url, identity)?;
    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, tls_connector)
            .await?;
    let (sink, mut source) = stream.split();

    // Unbounded so terminal output bursts never block the app under a lock.
    let (control_tx, control_rx) = mpsc::unbounded_channel::<Message>();
    let (carrier, envelopes_rx) = CarrierHandle::open();
    let writer = spawn_writer(sink, control_rx, envelopes_rx);
    let mut connection = RelayConnection::new(control_tx.clone(), identity, &intake, carrier);

    // Handlers run in the intake, not on this task: below, the loop only reads
    // and hands over, so no handler can stop the socket from being drained.
    let outcome: Result<(), RelayError> = async {
        let mut deadline = silence_deadline(DEFAULT_HEARTBEAT_INTERVAL_S);
        loop {
            let Ok(next) = tokio::time::timeout(deadline, source.next()).await else {
                return Err(RelayError::Silent(deadline));
            };
            let Some(message) = next else { break };
            let text = match message? {
                Message::Text(text) => text,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if let Some(next_deadline) = connection.accept(&msg).await {
                deadline = next_deadline;
            }
        }
        Ok(())
    }
    .await;

    connection.close();
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
                    Some(outbound) => relay_message(&outbound),
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
/// heartbeat that keeps the relay believing in the device.
struct RelayConnection<'a> {
    control_tx: mpsc::UnboundedSender<Message>,
    identity: &'a DeviceIdentity,
    intake: &'a FrameIntake,
    carrier: CarrierHandle,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
}

impl<'a> RelayConnection<'a> {
    fn new(
        control_tx: mpsc::UnboundedSender<Message>,
        identity: &'a DeviceIdentity,
        intake: &'a FrameIntake,
        carrier: CarrierHandle,
    ) -> Self {
        RelayConnection {
            control_tx,
            identity,
            intake,
            carrier,
            heartbeat: None,
        }
    }

    /// One relay message, honoured. Returns the silence deadline the message
    /// sets, which only `authenticated` does — it carries the interval.
    async fn accept(&mut self, msg: &Value) -> Option<Duration> {
        match msg.get("type").and_then(Value::as_str).unwrap_or("") {
            "authenticated" => return Some(self.authenticated(msg)),
            "session_init" => {
                if let Err(e) = self.open_session(msg) {
                    log_protocol_error(&e);
                }
            }
            "e2ee_envelope" => {
                if let Err(e) = self.take_envelope(msg).await {
                    log_protocol_error(&e);
                }
            }
            "session_closed" => {
                if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                    self.intake.close_session(session_id, &self.carrier);
                }
            }
            // "response"/"error"/unknown: nothing for the device to do here.
            _ => {}
        }
        None
    }

    /// The relay took the signed challenge: upload the transport public key
    /// clients wrap session keys to, and start heartbeating at its interval.
    fn authenticated(&mut self, msg: &Value) -> Duration {
        let interval = msg
            .get("heartbeat_interval_s")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_S);
        send(
            &self.control_tx,
            json!({
                "type": "transport_key",
                "transport_public_key": self.identity.transport.public_key_b64,
            }),
        );
        self.heartbeat = Some(spawn_heartbeat(self.control_tx.clone(), interval));
        silence_deadline(interval)
    }

    /// A client opened a session: unwrap its session key, register it against
    /// this carrier and prove receipt with an encrypted `session_accept`.
    fn open_session(&self, msg: &Value) -> Result<(), RelayError> {
        let session_id = field_str(msg, "session_id")?;
        let init: SessionInit = serde_json::from_value(
            msg.get("session_init")
                .cloned()
                .ok_or_else(|| RelayError::Protocol("session_init payload missing".into()))?,
        )
        .map_err(|e| RelayError::Protocol(format!("bad session_init: {e}")))?;

        let opened = transport::open_session_init(&self.identity.transport.private_key_b64, &init)?;
        self.intake
            .open(&session_id, opened.session_key_b64.clone(), &self.carrier)?;

        let accept = transport::build_session_accept(
            &opened.session_key_b64,
            &session_id,
            &transport::session_route(&session_id),
            None,
        )?;
        send(
            &self.control_tx,
            json!({
                "type": "session_accept",
                "session_id": session_id,
                "envelope": accept,
            }),
        );
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

    /// The socket is gone: this carrier carries nothing more, the heartbeat that
    /// fed it stops with it, and its hold on the writer's queue goes with it.
    fn close(mut self) {
        self.intake.close_carrier(&self.carrier);
        if let Some(task) = self.heartbeat.take() {
            task.abort();
        }
    }
}

fn spawn_heartbeat(
    control_tx: mpsc::UnboundedSender<Message>,
    interval_s: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s.max(1)));
        loop {
            ticker.tick().await;
            let heartbeat =
                control_tx.send(Message::Text(json!({"type": "heartbeat"}).to_string()));
            let ping = control_tx.send(Message::Ping(Vec::new()));
            if heartbeat.is_err() || ping.is_err() {
                break;
            }
        }
    })
}

/// The relay's wire wrapper: the one thing this carrier adds to an envelope.
fn relay_message(outbound: &OutboundEnvelope) -> Message {
    Message::Text(
        json!({
            "type": "e2ee_envelope",
            "session_id": outbound.session_id(),
            "envelope": outbound.envelope(),
        })
        .to_string(),
    )
}

fn send(control_tx: &mpsc::UnboundedSender<Message>, value: Value) {
    let _ = control_tx.send(Message::Text(value.to_string()));
}

fn field_str(msg: &Value, key: &str) -> Result<String, RelayError> {
    msg.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RelayError::Protocol(format!("{key} missing")))
}

/// A frame the device could not honour. Never fatal to the socket, and never
/// silent: a refused `session_init` or a frame for a session nobody knows is
/// what a browser sees as a call that never came back.
fn log_protocol_error(err: &RelayError) {
    eprintln!("relay protocol error: {err}");
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

        let Message::Text(text) = relay_message(&outbound) else {
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
