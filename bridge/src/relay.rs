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

pub use crate::carrier::FrameHandler;
use crate::carrier::{CarrierError, CarrierHandle, FrameIntake, OutboundEnvelope, SessionRegistry};
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
/// silently pick native-tls). Decrypted client request frames are passed to
/// `handler`; its returned payload is encrypted and sent back as an
/// `e2ee_envelope`.
pub async fn run(
    url: &str,
    identity: &DeviceIdentity,
    handler: FrameHandler,
) -> Result<(), RelayError> {
    run_with_connector(url, identity, handler, None).await
}

/// [`run`], with an explicit TLS connector. `None` uses the default (rustls +
/// webpki roots for `wss://`, plain TCP for `ws` URLs); tests inject
/// `Connector::Rustls` trusting a self-signed root to exercise real TLS locally.
///
/// The sessions minted here go in a registry of this call's own, so they end
/// with this socket. A device whose sessions outlive a socket passes its own
/// intake to [`run_with_intake`].
pub async fn run_with_connector(
    url: &str,
    identity: &DeviceIdentity,
    handler: FrameHandler,
    tls_connector: Option<tokio_tungstenite::Connector>,
) -> Result<(), RelayError> {
    let intake = FrameIntake::new(Arc::new(SessionRegistry::new()), handler);
    run_with_intake(url, identity, intake, tls_connector).await
}

/// [`run`] against an intake that outlives the socket: this connection is one
/// carrier of the device's sessions, and every session it mints is reachable
/// from every other carrier that intake serves. When the socket ends, the
/// carrier is released — the sessions that rode nothing else end with it.
pub async fn run_with_intake(
    url: &str,
    identity: &DeviceIdentity,
    intake: Arc<FrameIntake>,
    tls_connector: Option<tokio_tungstenite::Connector>,
) -> Result<(), RelayError> {
    let request = auth_request(url, identity)?;
    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, tls_connector)
            .await?;
    let (mut sink, mut source) = stream.split();

    // One writer owns the sink; everything else queues to it. Two queues meet
    // here: the relay's own control messages, already wire-shaped, and the
    // envelopes the app pushes to sessions, which know nothing of this wire.
    // Unbounded so pushes (terminal output bursts) never block the app under a
    // lock.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let (envelopes_tx, mut envelopes_rx) = mpsc::unbounded_channel::<OutboundEnvelope>();
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                control = out_rx.recv() => match control {
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
    });

    let carrier = CarrierHandle::new(envelopes_tx.clone());
    let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;

    // Handlers run in the intake, not on this task: below, the loop only reads
    // and hands over, so no handler can stop the socket from being drained.
    let outcome: Result<(), RelayError> = async {
        let mut deadline = silence_deadline(DEFAULT_HEARTBEAT_INTERVAL_S);
        loop {
            let Ok(next) = tokio::time::timeout(deadline, source.next()).await else {
                return Err(RelayError::Silent(deadline));
            };
            let Some(message) = next else { break };
            let message = message?;
            let text = match message {
                Message::Text(t) => t,
                Message::Ping(_) | Message::Pong(_) => continue,
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                continue;
            };

            match msg.get("type").and_then(Value::as_str).unwrap_or("") {
                "authenticated" => {
                    let interval = msg
                        .get("heartbeat_interval_s")
                        .and_then(Value::as_u64)
                        .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_S);
                    deadline = silence_deadline(interval);
                    // Upload our transport public key so clients can wrap to it.
                    send(
                        &out_tx,
                        json!({
                            "type": "transport_key",
                            "transport_public_key": identity.transport.public_key_b64,
                        }),
                    );
                    heartbeat = Some(spawn_heartbeat(out_tx.clone(), interval));
                }
                "session_init" => {
                    if let Err(e) = handle_session_init(&out_tx, identity, &intake, &carrier, &msg)
                    {
                        tracing_protocol_error(&e);
                    }
                }
                "e2ee_envelope" => {
                    if let Err(e) = handle_envelope(&intake, &carrier, &msg).await {
                        tracing_protocol_error(&e);
                    }
                }
                // The relay says the browser behind this session is gone: this
                // carrier stops carrying it, and the session ends with it unless
                // another carrier is still riding.
                "session_closed" => {
                    if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                        intake.close_session(session_id, carrier.id()).await;
                    }
                }
                // "response"/"error"/unknown: nothing for the device to do here.
                _ => {}
            }
        }
        Ok(())
    }
    .await;

    intake.close_carrier(carrier.id()).await;
    abort_heartbeat(heartbeat);
    drop(out_tx);
    drop(envelopes_tx);
    let _ = writer.await;
    outcome
}

fn abort_heartbeat(heartbeat: Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = heartbeat {
        task.abort();
    }
}

/// A client opened a session: unwrap its session key, register it against this
/// carrier and prove receipt with an encrypted `session_accept`.
fn handle_session_init(
    out_tx: &mpsc::UnboundedSender<Message>,
    identity: &DeviceIdentity,
    intake: &FrameIntake,
    carrier: &CarrierHandle,
    msg: &Value,
) -> Result<(), RelayError> {
    let session_id = field_str(msg, "session_id")?;
    let init: SessionInit = serde_json::from_value(
        msg.get("session_init")
            .cloned()
            .ok_or_else(|| RelayError::Protocol("session_init payload missing".into()))?,
    )
    .map_err(|e| RelayError::Protocol(format!("bad session_init: {e}")))?;

    let opened = transport::open_session_init(&identity.transport.private_key_b64, &init)?;
    intake.open(&session_id, opened.session_key_b64.clone(), carrier)?;

    let accept = transport::build_session_accept(
        &opened.session_key_b64,
        &session_id,
        &format!("session:{session_id}"),
        None,
    )?;
    send(
        out_tx,
        json!({
            "type": "session_accept",
            "session_id": session_id,
            "envelope": accept,
        }),
    );
    Ok(())
}

/// A client sent an encrypted frame on this carrier. Everything past parsing it
/// off the relay wire — the key, the close rule, dispatch — belongs to the
/// intake, which answers to every carrier alike.
async fn handle_envelope(
    intake: &FrameIntake,
    carrier: &CarrierHandle,
    msg: &Value,
) -> Result<(), RelayError> {
    let envelope: Envelope = serde_json::from_value(
        msg.get("envelope")
            .cloned()
            .ok_or_else(|| RelayError::Protocol("envelope missing".into()))?,
    )
    .map_err(|e| RelayError::Protocol(format!("bad envelope: {e}")))?;
    intake.accept(envelope, carrier).await?;
    Ok(())
}

fn spawn_heartbeat(
    out_tx: mpsc::UnboundedSender<Message>,
    interval_s: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s.max(1)));
        loop {
            ticker.tick().await;
            let heartbeat = out_tx.send(Message::Text(json!({"type": "heartbeat"}).to_string()));
            let ping = out_tx.send(Message::Ping(Vec::new()));
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
            "session_id": outbound.session_id,
            "envelope": outbound.envelope,
        })
        .to_string(),
    )
}

fn send(out_tx: &mpsc::UnboundedSender<Message>, value: Value) {
    let _ = out_tx.send(Message::Text(value.to_string()));
}

fn field_str(msg: &Value, key: &str) -> Result<String, RelayError> {
    msg.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RelayError::Protocol(format!("{key} missing")))
}

fn tracing_protocol_error(err: &RelayError) {
    // Protocol errors on a single frame must not kill the connection; a real
    // build wires this to `tracing`. Kept minimal here.
    let _ = err;
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    /// The wire wrapper is the relay carrier's whole contribution: an envelope
    /// goes out as `{"type":"e2ee_envelope",…}` and nothing above the carrier
    /// boundary has to know that.
    #[test]
    fn the_writer_wraps_an_outbound_envelope_for_the_relay_wire() {
        let outbound = OutboundEnvelope {
            session_id: "s-1".into(),
            envelope: Envelope {
                version: 1,
                session_id: "s-1".into(),
                route_to: "session:s-1".into(),
                nonce: "bm9uY2U=".into(),
                ciphertext: "Y2lwaGVy".into(),
            },
        };

        let Message::Text(text) = relay_message(&outbound) else {
            panic!("the relay carries text frames");
        };
        let wire: Value = serde_json::from_str(&text).expect("the wrapper is JSON");
        assert_eq!(wire["type"], "e2ee_envelope");
        assert_eq!(wire["session_id"], "s-1");
        assert_eq!(
            wire["envelope"],
            serde_json::to_value(&outbound.envelope).unwrap(),
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
