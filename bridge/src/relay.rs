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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::Message;

use crate::transport::{self, Envelope, Frame, KeyPairB64, OuterFields, SessionInit};

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

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<Message>,
}

impl SessionSender {
    /// The session this sender targets.
    pub fn session_id(&self) -> &str {
        &self.session_id
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

    /// Encrypt `payload` as an inner frame and send it to the client as an
    /// `e2ee_envelope`. Returns false once the connection is gone (so the app can
    /// drop the stale sender).
    pub fn push(&self, payload: Value) -> bool {
        let envelope = match transport::encrypt_frame(
            &self.session_key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: format!("session:{}", self.session_id),
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
        self.out
            .send(Message::Text(
                json!({ "type": "e2ee_envelope", "session_id": self.session_id, "envelope": envelope })
                    .to_string(),
            ))
            .is_ok()
    }
}

/// Handles a decrypted request frame. Receives a [`SessionSender`] (so it can
/// register the session for server-initiated pushes) and returns the response
/// payload to send back.
pub type FrameHandler = Arc<dyn Fn(SessionSender, Frame) -> Value + Send + Sync>;

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
pub async fn run_with_connector(
    url: &str,
    identity: &DeviceIdentity,
    handler: FrameHandler,
    tls_connector: Option<tokio_tungstenite::Connector>,
) -> Result<(), RelayError> {
    let request = auth_request(url, identity)?;
    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, tls_connector)
            .await?;
    let (mut sink, mut source) = stream.split();

    // One writer owns the sink; everything else queues messages to it. Unbounded
    // so pushes (terminal output bursts) never block the app under a lock.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // session_id → session_key (base64). One reader task, so no lock needed.
    let mut sessions: HashMap<String, String> = HashMap::new();
    let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(message) = source.next().await {
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
                    .unwrap_or(30);
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
                if let Err(e) = handle_session_init(&out_tx, identity, &mut sessions, &msg) {
                    tracing_protocol_error(&e);
                }
            }
            "e2ee_envelope" => {
                if let Err(e) = handle_envelope(&out_tx, &sessions, &msg, &handler) {
                    tracing_protocol_error(&e);
                }
            }
            // "response"/"error"/unknown: nothing for the device to do here.
            _ => {}
        }
    }

    if let Some(h) = heartbeat {
        h.abort();
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// A client opened a session: unwrap its session key and prove receipt with an
/// encrypted `session_accept`.
fn handle_session_init(
    out_tx: &mpsc::UnboundedSender<Message>,
    identity: &DeviceIdentity,
    sessions: &mut HashMap<String, String>,
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
    sessions.insert(session_id.clone(), opened.session_key_b64.clone());

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

/// A client sent an encrypted frame: decrypt it, hand the inner request to the
/// application along with a [`SessionSender`], and send the response back.
fn handle_envelope(
    out_tx: &mpsc::UnboundedSender<Message>,
    sessions: &HashMap<String, String>,
    msg: &Value,
    handler: &FrameHandler,
) -> Result<(), RelayError> {
    let session_id = field_str(msg, "session_id")?;
    let session_key = sessions
        .get(&session_id)
        .ok_or_else(|| RelayError::Protocol(format!("no session key for {session_id}")))?;
    let envelope: Envelope = serde_json::from_value(
        msg.get("envelope")
            .cloned()
            .ok_or_else(|| RelayError::Protocol("envelope missing".into()))?,
    )
    .map_err(|e| RelayError::Protocol(format!("bad envelope: {e}")))?;

    let frame = transport::decrypt_envelope(session_key, &envelope)?;
    // `close` frames end the conversation; nothing to answer.
    if frame.frame_type == "close" {
        return Ok(());
    }

    let sender = SessionSender {
        session_id: session_id.clone(),
        session_key: session_key.clone(),
        out: out_tx.clone(),
    };
    // The response rides the same push channel; the app may also have pushed
    // server-initiated frames during the call (e.g. an initial terminal flush).
    let response_payload = handler(sender.clone(), frame);
    sender.push(response_payload);
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
            if out_tx
                .send(Message::Text(json!({"type": "heartbeat"}).to_string()))
                .is_err()
            {
                break;
            }
        }
    })
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
