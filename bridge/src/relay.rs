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
//! anything this socket reports.
//!
//! Frame flow, per the relay's `/ws/device` protocol:
//! - relay → `{"type":"authenticated","heartbeat_interval_s":N}`
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

use crate::carrier::{self, CarrierError, CarrierHandle, FrameIntake, OutboundEnvelope};
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
    let mut connection = RelayConnection::new(control_tx.clone(), &intake, carrier);

    // Handlers run in the intake, not on this task: below, the loop only reads
    // and hands over, so no handler can stop the socket from being drained.
    let outcome: Result<(), RelayError> = async {
        loop {
            let Ok(next) = tokio::time::timeout(connection.deadline, source.next()).await else {
                return Err(RelayError::Silent(connection.deadline));
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
            connection.accept(&msg).await;
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
    /// How long the relay may stay silent before this socket is given up on;
    /// the interval `authenticated` carries sets it.
    deadline: Duration,
}

impl<'a> RelayConnection<'a> {
    fn new(
        control_tx: mpsc::UnboundedSender<Message>,
        intake: &'a FrameIntake,
        carrier: CarrierHandle,
    ) -> Self {
        RelayConnection {
            control_tx,
            intake,
            carrier,
            heartbeat: None,
            deadline: silence_deadline(DEFAULT_HEARTBEAT_INTERVAL_S),
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
                    self.intake.close_session(session_id, &self.carrier);
                }
            }
            // "response"/"error"/unknown: nothing for the device to do here.
            _ => {}
        }
    }

    /// The relay took the signed challenge: start heartbeating at its interval.
    /// Nothing else is said — this socket keeps the device findable and carries
    /// the sessions a browser mints on it, and that is all.
    fn authenticated(&mut self, msg: &Value) {
        let interval = msg
            .get("heartbeat_interval_s")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL_S);
        self.heartbeat = Some(spawn_heartbeat(self.control_tx.clone(), interval));
        self.deadline = silence_deadline(interval);
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
    /// with it.
    fn drop(&mut self) {
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
