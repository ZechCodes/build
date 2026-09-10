//! The dev relay's decision logic — pure and unit-testable, separate from the socket
//! wiring in `bin/relay.rs`.
//!
//! The relay authenticates devices (Ed25519 challenge vs the api's device record),
//! identifies browsers (a gateway token the api minted), and routes opaque E2EE
//! envelopes between them — never decrypting anything. Crucially it enforces
//! **ownership**: a browser may only open a session to a device its own user owns.
//!
//! State is a registry of connected devices (keyed by `device_id`, each carrying its
//! `owner_user_id`) and clients (each carrying its `user_id`), plus a `session_id →
//! (client, device)` map so envelopes route by session like the production relay.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::transport;

/// The signed-challenge path (`{ts}.GET./ws/device`) the device proves.
pub const AUTH_PATH: &str = "/ws/device";
/// Allowed clock skew between the device's timestamp and the relay.
pub const AUTH_SKEW: Duration = Duration::from_secs(60);
/// How long the [`ReplayGuard`] must remember a seen challenge. A device's clock
/// may run up to `AUTH_SKEW` ahead, and its signature then stays valid for
/// another `AUTH_SKEW` — so the guard's memory must span **both** windows, or a
/// captured challenge becomes replayable the moment the guard forgets it.
pub const REPLAY_TTL: Duration = Duration::from_secs(2 * AUTH_SKEW.as_secs());

/// Hard cap on a single WebSocket message/frame the relay will buffer. Envelopes are
/// base64 ciphertext of terminal chunks and diffs; anything past this is abusive.
pub const MAX_WS_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// The heartbeat cadence the relay advertises to a device in its `authenticated`
/// greeting. The device promises a `{"type":"heartbeat"}` frame this often; the
/// default liveness deadline below is derived from it, so the advertisement and
/// the enforcement can never drift apart.
pub const HEARTBEAT_INTERVAL_S: u64 = 30;

/// Hard cap on the bytes queued to one peer's writer. A browser that stops reading
/// (backgrounded tab, stalled TCP) while its device streams terminal output would
/// otherwise grow an unbounded queue until the relay OOMs. Past the cap, sends to
/// that peer fail — the snapshot+cursor resume design recovers the lost frames.
pub const MAX_OUTBOUND_QUEUE_BYTES: usize = 32 * 1024 * 1024;

/// Why an [`Outbound::send`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundSendError {
    /// The peer's writer is gone (connection closed).
    Closed,
    /// The peer's queue is over [`MAX_OUTBOUND_QUEUE_BYTES`]: it reads too slowly.
    Overflow,
}

/// A connected peer's outbound channel: text payloads only (the bin wraps them in
/// WebSocket frames), with a byte-bounded queue so one slow reader can never grow
/// relay memory without limit. Decoupled from tungstenite so this stays testable.
#[derive(Clone)]
pub struct Outbound {
    tx: mpsc::UnboundedSender<String>,
    queued_bytes: Arc<AtomicUsize>,
}

impl Outbound {
    /// Queue `text` to the peer's writer. Fails when the writer is gone or the
    /// peer has [`MAX_OUTBOUND_QUEUE_BYTES`] already queued (a laggard).
    pub fn send(&self, text: String) -> Result<(), OutboundSendError> {
        if self.queued_bytes.load(Ordering::Acquire) >= MAX_OUTBOUND_QUEUE_BYTES {
            return Err(OutboundSendError::Overflow);
        }
        self.queued_bytes.fetch_add(text.len(), Ordering::AcqRel);
        self.tx.send(text).map_err(|refused| {
            self.queued_bytes
                .fetch_sub(refused.0.len(), Ordering::AcqRel);
            OutboundSendError::Closed
        })
    }
}

/// The writer half of an [`Outbound`] channel; dequeuing releases queue budget.
pub struct OutboundReceiver {
    rx: mpsc::UnboundedReceiver<String>,
    queued_bytes: Arc<AtomicUsize>,
}

impl OutboundReceiver {
    pub async fn recv(&mut self) -> Option<String> {
        let text = self.rx.recv().await;
        if let Some(text) = &text {
            self.queued_bytes.fetch_sub(text.len(), Ordering::AcqRel);
        }
        text
    }

    pub fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError> {
        let text = self.rx.try_recv()?;
        self.queued_bytes.fetch_sub(text.len(), Ordering::AcqRel);
        Ok(text)
    }
}

/// A fresh byte-bounded outbound channel for one peer connection.
pub fn outbound_channel() -> (Outbound, OutboundReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let queued_bytes = Arc::new(AtomicUsize::new(0));
    (
        Outbound {
            tx,
            queued_bytes: Arc::clone(&queued_bytes),
        },
        OutboundReceiver { rx, queued_bytes },
    )
}

/// The relay bin's configuration, resolved from the environment.
///
/// - `RELAY_PORT` — listen port (default 8799; `0` binds an ephemeral port).
/// - `API_INTERNAL_URL` — the api's internal base URL (falls back to the legacy
///   `RELAY_API_URL`, then `http://127.0.0.1:8080` for dev).
/// - `RELAY_INTERNAL_SECRET` — sent as `X-Internal-Secret` on every api call; unset or
///   blank means dev mode where the api trusts localhost instead.
/// - `RELAY_DEVICE_LIVENESS_S` — sever a device that sends no frame, or a peer of
///   either kind that answers no ping, for this long (default
///   `3 × HEARTBEAT_INTERVAL_S`). A wedged bridge that stops reading and writing
///   must be deregistered and reported offline, not stay "online" forever; a
///   browser that is gone while a load balancer keeps its TCP connection
///   established must not stay a client forever either.
/// - `RELAY_WRITE_STALL_S` — a single WebSocket write blocked this long means the
///   peer stopped reading; the connection is severed (default 30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayConfig {
    pub port: u16,
    pub api_url: String,
    pub internal_secret: Option<String>,
    pub device_liveness_timeout: Duration,
    pub write_stall_timeout: Duration,
}

impl RelayConfig {
    /// Resolve the config through a lookup function (`std::env::var` in the bin,
    /// a map in tests). Fails fast on an unparseable port instead of masking it
    /// with the default.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<RelayConfig, String> {
        let port = match lookup("RELAY_PORT") {
            Some(raw) => raw
                .parse::<u16>()
                .map_err(|_| format!("RELAY_PORT is not a valid port: {raw:?}"))?,
            None => 8799,
        };
        let api_url = lookup("API_INTERNAL_URL")
            .or_else(|| lookup("RELAY_API_URL"))
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
            .trim_end_matches('/')
            .to_string();
        let internal_secret = lookup("RELAY_INTERNAL_SECRET").filter(|s| !s.is_empty());
        let device_liveness_timeout =
            positive_seconds(&lookup, "RELAY_DEVICE_LIVENESS_S", 3 * HEARTBEAT_INTERVAL_S)?;
        let write_stall_timeout = positive_seconds(&lookup, "RELAY_WRITE_STALL_S", 30)?;
        Ok(RelayConfig {
            port,
            api_url,
            internal_secret,
            device_liveness_timeout,
            write_stall_timeout,
        })
    }
}

/// Parse an env var as a positive whole number of seconds, defaulting when unset.
/// Zero is rejected along with garbage: a zero timeout severs every peer instantly,
/// which is never what a deployment meant.
fn positive_seconds(
    lookup: impl Fn(&str) -> Option<String>,
    name: &str,
    default_secs: u64,
) -> Result<Duration, String> {
    let secs = match lookup(name) {
        Some(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|secs| *secs > 0)
            .ok_or_else(|| format!("{name} is not a positive number of seconds: {raw:?}"))?,
        None => default_secs,
    };
    Ok(Duration::from_secs(secs))
}

/// Whether a raw request prefix is a plain `GET /health` probe (kubelet, LB) rather
/// than a WebSocket upgrade. Matched on the request line only, before any handshake.
pub fn is_health_request(prefix: &[u8]) -> bool {
    const REQUEST_LINE: &[u8] = b"GET /health";
    prefix.strip_prefix(REQUEST_LINE).is_some_and(|rest| {
        // The path must end exactly there: next byte is the version separator,
        // a query string, or (leniently) the end of the request line.
        matches!(rest.first(), Some(b' ') | Some(b'?') | Some(b'\r'))
    })
}

/// Parse the browser's mandatory first frame `{"type":"authenticate","token":...}`.
/// Anything else — wrong type, missing token, not JSON — is `None` and the bin
/// closes the connection: unauthenticated clients get exactly one frame.
pub fn parse_authenticate_token(text: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(text).ok()?;
    if msg.get("type").and_then(Value::as_str) != Some("authenticate") {
        return None;
    }
    msg.get("token").and_then(Value::as_str).map(str::to_string)
}

/// The device a `session_init` frame targets. The documented contract is the outer
/// `"route_to":"device:<id>"`; older harnesses put the id only inside the sealed-around
/// `session_init.device_id`, so that remains a fallback. A `route_to` that is present
/// but not `device:`-shaped is an error (`None`), never a silent fallback.
pub fn session_target_device(msg: &Value) -> Option<String> {
    if let Some(route_to) = msg.get("route_to").and_then(Value::as_str) {
        return route_to
            .strip_prefix("device:")
            .filter(|id| !id.is_empty())
            .map(str::to_string);
    }
    msg.get("session_init")
        .and_then(|init| init.get("device_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The captured device auth from the WS upgrade headers.
#[derive(Debug, Clone)]
pub struct DeviceAuth {
    pub device_id: String,
    pub timestamp: String,
    pub signature: String,
}

/// The device record the relay fetches from the api's internal endpoint.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub identity_public_key_b64: String,
    pub approved: bool,
    pub owner_user_id: Option<String>,
}

/// The verdict of authenticating a device connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    Ok { owner_user_id: String },
    Reject(String),
}

/// Authenticate a device's WS upgrade: pure, no I/O. Rejects unapproved/ownerless
/// devices, stale timestamps (replay window), and bad signatures.
pub fn authorize_device(
    auth: &DeviceAuth,
    record: &DeviceRecord,
    now_unix: u64,
    skew: Duration,
) -> AuthOutcome {
    if !record.approved {
        return AuthOutcome::Reject("device not approved".into());
    }
    let Some(owner) = record.owner_user_id.clone() else {
        return AuthOutcome::Reject("device has no owner".into());
    };
    let Ok(ts) = auth.timestamp.parse::<u64>() else {
        return AuthOutcome::Reject("bad timestamp".into());
    };
    if now_unix.abs_diff(ts) > skew.as_secs() {
        return AuthOutcome::Reject("timestamp outside allowed skew".into());
    }
    let challenge = format!("{ts}.GET.{AUTH_PATH}");
    match transport::verify_message_b64(
        &record.identity_public_key_b64,
        challenge.as_bytes(),
        &auth.signature,
    ) {
        Ok(()) => AuthOutcome::Ok {
            owner_user_id: owner,
        },
        Err(_) => AuthOutcome::Reject("signature verification failed".into()),
    }
}

/// Rejects a replayed device upgrade: the signed challenge has no nonce, so within the
/// skew window an attacker who captured the headers could re-present them. Tracks the
/// `(device_id, timestamp, signature)` tuples seen recently and refuses duplicates.
pub struct ReplayGuard {
    seen: HashMap<(String, String, String), Instant>,
    ttl: Duration,
}

impl ReplayGuard {
    pub fn new(ttl: Duration) -> Self {
        ReplayGuard {
            seen: HashMap::new(),
            ttl,
        }
    }

    /// Record the tuple; returns `false` if it was already seen within the TTL (replay).
    pub fn check_and_record(
        &mut self,
        device_id: &str,
        timestamp: &str,
        signature: &str,
        now: Instant,
    ) -> bool {
        self.evict_expired(now);
        let key = (
            device_id.to_string(),
            timestamp.to_string(),
            signature.to_string(),
        );
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, now);
        true
    }

    pub fn evict_expired(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.seen
            .retain(|_, seen_at| now.duration_since(*seen_at) <= ttl);
    }
}

struct ConnectedDevice {
    owner_user_id: String,
    transport_key: Option<String>,
    out: Outbound,
    /// Which physical connection registered this entry — so a stale socket's late
    /// cleanup can't deregister a newer reconnection of the same device.
    conn_id: u64,
    /// Ends this connection's serve loop when a newer socket authenticates with
    /// the same device identity.
    supersede: tokio::sync::oneshot::Sender<()>,
}

struct ConnectedClient {
    user_id: String,
    out: Outbound,
}

struct Session {
    client_id: u64,
    device_id: String,
}

/// The result of registering a device connection: its connection id, any clients
/// whose sessions were severed because this is a reconnect of the same device, and
/// the owner's connected clients to push `device_online` to.
pub struct DeviceRegistration {
    pub conn_id: u64,
    pub superseded_conn_id: Option<u64>,
    pub superseded: tokio::sync::oneshot::Receiver<()>,
    pub displaced_clients: Vec<Outbound>,
    pub owner_clients: Vec<Outbound>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFrameRoute {
    Forwarded,
    NoRoute,
    StaleConnection,
}

/// The relay's live routing table. Not thread-safe by itself; the bin wraps it in a
/// mutex. Methods mutate `&mut self` and never touch the network.
#[derive(Default)]
pub struct RelayState {
    devices: HashMap<String, ConnectedDevice>,
    clients: HashMap<u64, ConnectedClient>,
    sessions: HashMap<String, Session>,
    next_client_id: u64,
    next_device_conn_id: u64,
}

impl RelayState {
    pub fn new() -> Self {
        Self::default()
    }

    // ----- devices -----------------------------------------------------------

    /// Register an authenticated device (a reconnect replaces the previous entry).
    /// Any sessions still routed to the old connection are severed — their keys died
    /// with the old device process — and the affected clients are returned so the bin
    /// can tell them to re-handshake. The `conn_id` goes back to [`remove_device`] so
    /// a stale socket's late cleanup can't deregister this newer connection.
    pub fn add_device(
        &mut self,
        device_id: &str,
        owner_user_id: &str,
        out: Outbound,
    ) -> DeviceRegistration {
        let conn_id = self.next_device_conn_id;
        self.next_device_conn_id += 1;
        let (supersede, superseded) = tokio::sync::oneshot::channel();

        let displaced_ids: Vec<u64> = self
            .sessions
            .values()
            .filter(|s| s.device_id == device_id)
            .map(|s| s.client_id)
            .collect();
        self.sessions.retain(|_, s| s.device_id != device_id);
        let mut seen = std::collections::HashSet::new();
        let displaced_clients = displaced_ids
            .into_iter()
            .filter(|id| seen.insert(*id))
            .filter_map(|id| self.clients.get(&id).map(|c| c.out.clone()))
            .collect();

        let previous = self.devices.insert(
            device_id.to_string(),
            ConnectedDevice {
                owner_user_id: owner_user_id.to_string(),
                transport_key: None,
                out,
                conn_id,
                supersede,
            },
        );
        let superseded_conn_id = previous.map(|previous| {
            let previous_conn_id = previous.conn_id;
            let _ = previous.supersede.send(());
            previous_conn_id
        });
        DeviceRegistration {
            conn_id,
            superseded_conn_id,
            superseded,
            displaced_clients,
            owner_clients: self.client_outbounds_for_user(owner_user_id),
        }
    }

    /// Record a device's transport key and enqueue its notice to every owner client.
    /// Generation validation and enqueue happen in this one state operation, so a
    /// replacement cannot interleave and let a stale key escape after handoff.
    pub fn publish_device_transport_key(
        &mut self,
        device_id: &str,
        conn_id: u64,
        key: &str,
    ) -> Option<usize> {
        let owner = match self.devices.get_mut(device_id) {
            Some(device) if device.conn_id == conn_id => {
                device.transport_key = Some(key.to_string());
                device.owner_user_id.clone()
            }
            _ => return None,
        };
        let clients = self.client_outbounds_for_user(&owner);
        let notice = json!({
            "type": "device_key",
            "device_id": device_id,
            "transport_public_key": key,
        })
        .to_string();
        for client in &clients {
            let _ = client.send(notice.clone());
        }
        Some(clients.len())
    }

    /// Remove a device and drop any sessions routed to it — but only if the entry
    /// still belongs to `conn_id`, so a stale socket's late cleanup is a no-op after
    /// the device reconnected. `None` means exactly that stale no-op: the caller must
    /// not report the device offline (it is live on a newer connection). `Some`
    /// carries the outbounds of the owner's connected clients so the bin can push a
    /// `device_offline` notice — the browser's cue to degrade gracefully instead of
    /// hanging on a dead session.
    pub fn remove_device(&mut self, device_id: &str, conn_id: u64) -> Option<Vec<Outbound>> {
        match self.devices.get(device_id) {
            Some(device) if device.conn_id == conn_id => {}
            _ => return None,
        }
        let device = self.devices.remove(device_id).expect("checked above");
        self.sessions.retain(|_, s| s.device_id != device_id);
        Some(self.client_outbounds_for_user(&device.owner_user_id))
    }

    /// The transport keys to advertise to a freshly-connected client: every device the
    /// user owns that has uploaded one. Returns `(device_id, transport_key)`.
    pub fn device_keys_for_user(&self, user_id: &str) -> Vec<(String, String)> {
        self.devices
            .iter()
            .filter(|(_, d)| d.owner_user_id == user_id)
            .filter_map(|(id, d)| d.transport_key.clone().map(|k| (id.clone(), k)))
            .collect()
    }

    // ----- clients -----------------------------------------------------------

    /// Register an authenticated browser client; returns its connection id.
    pub fn add_client(&mut self, user_id: &str, out: Outbound) -> u64 {
        let client_id = self.next_client_id;
        self.next_client_id += 1;
        self.clients.insert(
            client_id,
            ConnectedClient {
                user_id: user_id.to_string(),
                out,
            },
        );
        client_id
    }

    /// Remove a client and drop its sessions. Returns `(session_id, device outbound)`
    /// for every severed session whose device is still connected, so the bin can send
    /// the device a `session_closed` notice — otherwise the device keeps encrypting
    /// terminal output into sessions nobody will ever read again.
    pub fn remove_client(&mut self, client_id: u64) -> Vec<(String, Outbound)> {
        self.clients.remove(&client_id);
        let severed: Vec<(String, Outbound)> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.client_id == client_id)
            .filter_map(|(session_id, s)| {
                self.devices
                    .get(&s.device_id)
                    .map(|d| (session_id.clone(), d.out.clone()))
            })
            .collect();
        self.sessions.retain(|_, s| s.client_id != client_id);
        severed
    }

    // ----- routing -----------------------------------------------------------

    /// Open a session from a client to a device, enforcing ownership. Returns the
    /// device's outbound to forward the `session_init` to, or `None` if the device is
    /// unknown or owned by a different user (the cross-account guard), or the
    /// `session_id` is already registered to a *different* live client — a colliding
    /// or hostile client must not be able to sever someone else's session by
    /// re-registering its id.
    pub fn open_session(
        &mut self,
        session_id: &str,
        client_id: u64,
        device_id: &str,
    ) -> Option<Outbound> {
        let user_id = self.clients.get(&client_id)?.user_id.clone();
        if let Some(existing) = self.sessions.get(session_id) {
            if existing.client_id != client_id {
                return None;
            }
        }
        let device = self.devices.get(device_id)?;
        if device.owner_user_id != user_id {
            return None;
        }
        let out = device.out.clone();
        self.sessions.insert(
            session_id.to_string(),
            Session {
                client_id,
                device_id: device_id.to_string(),
            },
        );
        Some(out)
    }

    /// The device outbound for a client's in-session frame — only if the session belongs
    /// to that client.
    pub fn device_out_for_client_frame(
        &self,
        session_id: &str,
        client_id: u64,
    ) -> Option<Outbound> {
        let session = self.sessions.get(session_id)?;
        if session.client_id != client_id {
            return None;
        }
        self.devices.get(&session.device_id).map(|d| d.out.clone())
    }

    /// Route one device frame to its session client. Generation validation and
    /// enqueue are atomic with respect to registration, preventing buffered frames
    /// from a superseded socket crossing the handoff boundary.
    pub fn route_device_frame(
        &mut self,
        session_id: &str,
        device_id: &str,
        conn_id: u64,
        text: String,
    ) -> DeviceFrameRoute {
        match self.devices.get(device_id) {
            Some(device) if device.conn_id == conn_id => {}
            Some(_) => return DeviceFrameRoute::StaleConnection,
            None => return DeviceFrameRoute::NoRoute,
        }
        let Some(session) = self.sessions.get(session_id) else {
            return DeviceFrameRoute::NoRoute;
        };
        if session.device_id != device_id {
            return DeviceFrameRoute::NoRoute;
        }
        let Some(client) = self.clients.get(&session.client_id) else {
            return DeviceFrameRoute::NoRoute;
        };
        if client.out.send(text).is_ok() {
            DeviceFrameRoute::Forwarded
        } else {
            DeviceFrameRoute::NoRoute
        }
    }

    pub fn device_connection_is_current(&self, device_id: &str, conn_id: u64) -> bool {
        self.devices
            .get(device_id)
            .is_some_and(|device| device.conn_id == conn_id)
    }

    fn client_outbounds_for_user(&self, user_id: &str) -> Vec<Outbound> {
        self.clients
            .values()
            .filter(|c| c.user_id == user_id)
            .map(|c| c.out.clone())
            .collect()
    }
}

/// The request-line prefix that marks a plain-HTTP health probe.
const HEALTH_REQUEST_PREFIX: &[u8] = b"GET /health";

/// Answer plain-HTTP `GET /health` probes (Kubernetes liveness/readiness) on the
/// relay's WebSocket port. Peeks at the incoming bytes without consuming them, so
/// a real WebSocket upgrade can proceed unchanged afterwards. Returns `true` when
/// the connection was a probe: the 200 response has been written and the caller
/// should drop the socket.
pub async fn handle_health_probe(tcp: &mut tokio::net::TcpStream) -> std::io::Result<bool> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    // "GET /health" plus one byte of lookahead so `/healthz` etc. never match.
    let mut peeked = [0u8; HEALTH_REQUEST_PREFIX.len() + 1];
    loop {
        let n = tcp.peek(&mut peeked).await?;
        if n == 0 {
            return Ok(false); // peer closed before sending a request line
        }
        let prefix_len = n.min(HEALTH_REQUEST_PREFIX.len());
        if peeked[..prefix_len] != HEALTH_REQUEST_PREFIX[..prefix_len] {
            return Ok(false); // diverges from `GET /health` → not a probe
        }
        if n == peeked.len() || peeked[..n].contains(&b'\r') {
            if is_health_request(&peeked[..n]) {
                break; // full prefix + terminator seen: this is a probe
            }
            return Ok(false); // e.g. `GET /healthz`
        }
        // Partial prefix: wait for more bytes, but never stall the accept path.
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let body = "ok";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(response.as_bytes()).await?;
    tcp.shutdown().await?;
    // Drain the (peeked, never consumed) request bytes until the peer closes, so
    // dropping the socket doesn't RST before the probe reads our response.
    let drain_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut discard = [0u8; 512];
    loop {
        match tokio::time::timeout_at(drain_deadline, tcp.read(&mut discard)).await {
            Ok(Ok(read)) if read > 0 => {}
            _ => break, // EOF, error, or deadline: the response is on the wire
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity;

    fn signed_auth(device_id: &str, ts: u64) -> (DeviceAuth, DeviceRecord) {
        let id = identity::generate("box");
        let challenge = format!("{ts}.GET.{AUTH_PATH}");
        let sig = transport::sign_message_b64(&id.identity_private_key_b64, challenge.as_bytes())
            .unwrap();
        (
            DeviceAuth {
                device_id: device_id.to_string(),
                timestamp: ts.to_string(),
                signature: sig,
            },
            DeviceRecord {
                identity_public_key_b64: id.identity_public_key_b64,
                approved: true,
                owner_user_id: Some("u1".to_string()),
            },
        )
    }

    #[test]
    fn authorize_device_accepts_valid_signature_in_window() {
        let (auth, record) = signed_auth("d1", 1_000_000);
        assert_eq!(
            authorize_device(&auth, &record, 1_000_010, AUTH_SKEW),
            AuthOutcome::Ok {
                owner_user_id: "u1".into()
            }
        );
    }

    #[test]
    fn authorize_device_rejects_unapproved() {
        let (auth, mut record) = signed_auth("d1", 1_000_000);
        record.approved = false;
        assert!(matches!(
            authorize_device(&auth, &record, 1_000_000, AUTH_SKEW),
            AuthOutcome::Reject(_)
        ));
    }

    #[test]
    fn authorize_device_rejects_missing_owner() {
        let (auth, mut record) = signed_auth("d1", 1_000_000);
        record.owner_user_id = None;
        assert!(matches!(
            authorize_device(&auth, &record, 1_000_000, AUTH_SKEW),
            AuthOutcome::Reject(_)
        ));
    }

    #[test]
    fn authorize_device_rejects_skewed_timestamp() {
        let (auth, record) = signed_auth("d1", 1_000_000);
        // 10 minutes later — well outside the 60s window.
        assert!(matches!(
            authorize_device(&auth, &record, 1_000_600, AUTH_SKEW),
            AuthOutcome::Reject(_)
        ));
    }

    #[test]
    fn authorize_device_rejects_bad_signature() {
        let (mut auth, record) = signed_auth("d1", 1_000_000);
        // Re-sign with a different key → signature no longer matches the record.
        let other = identity::generate("other");
        let challenge = format!("1000000.GET.{AUTH_PATH}");
        auth.signature =
            transport::sign_message_b64(&other.identity_private_key_b64, challenge.as_bytes())
                .unwrap();
        assert!(matches!(
            authorize_device(&auth, &record, 1_000_000, AUTH_SKEW),
            AuthOutcome::Reject(_)
        ));
    }

    #[test]
    fn replay_guard_rejects_duplicate_within_ttl() {
        let mut guard = ReplayGuard::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(guard.check_and_record("d1", "1000", "sigA", now));
        assert!(!guard.check_and_record("d1", "1000", "sigA", now), "replay");
    }

    #[test]
    fn replay_guard_allows_after_ttl() {
        let mut guard = ReplayGuard::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(guard.check_and_record("d1", "1000", "sigA", now));
        let later = now + Duration::from_secs(61);
        assert!(guard.check_and_record("d1", "1000", "sigA", later));
    }

    #[test]
    fn replay_guard_distinguishes_devices_and_sigs() {
        let mut guard = ReplayGuard::new(Duration::from_secs(60));
        let now = Instant::now();
        assert!(guard.check_and_record("d1", "1000", "sigA", now));
        assert!(guard.check_and_record("d2", "1000", "sigA", now));
        assert!(guard.check_and_record("d1", "1000", "sigB", now));
    }

    fn chan() -> (Outbound, OutboundReceiver) {
        outbound_channel()
    }

    #[test]
    fn replay_guard_ttl_covers_the_full_signature_validity_window() {
        // A device clock up to AUTH_SKEW ahead signs a timestamp that stays valid
        // for AUTH_SKEW after the guard's connect-time record — the guard must not
        // forget the tuple while the signature can still authenticate.
        assert!(
            REPLAY_TTL >= AUTH_SKEW * 2,
            "TTL must span both skew windows"
        );
        let mut guard = ReplayGuard::new(REPLAY_TTL);
        let now = Instant::now();
        assert!(guard.check_and_record("d1", "1000", "sigA", now));
        let just_past_skew = now + AUTH_SKEW + Duration::from_secs(1);
        assert!(
            !guard.check_and_record("d1", "1000", "sigA", just_past_skew),
            "still rejected while the signature could remain valid"
        );
        let past_validity = now + REPLAY_TTL + Duration::from_secs(1);
        assert!(guard.check_and_record("d1", "1000", "sigA", past_validity));
    }

    #[test]
    fn outbound_queue_bounds_bytes_and_recovers_as_the_reader_drains() {
        let (out, mut rx) = outbound_channel();
        let chunk = "x".repeat(MAX_OUTBOUND_QUEUE_BYTES / 2);
        out.send(chunk.clone()).unwrap();
        out.send(chunk.clone()).unwrap();
        // The queue is at the cap: a laggard's next frame is refused, not buffered.
        assert_eq!(out.send("y".into()), Err(OutboundSendError::Overflow));
        // Draining releases budget and sends flow again.
        assert!(rx.try_recv().is_ok());
        out.send("y".into()).unwrap();
    }

    #[test]
    fn outbound_send_fails_closed_when_the_writer_is_gone() {
        let (out, rx) = outbound_channel();
        drop(rx);
        assert_eq!(out.send("y".into()), Err(OutboundSendError::Closed));
    }

    #[test]
    fn open_session_routes_only_to_owner_device() {
        let mut state = RelayState::new();
        let (d1_out, _d1_rx) = chan();
        let (d2_out, _d2_rx) = chan();
        state.add_device("dev-u1", "u1", d1_out);
        state.add_device("dev-u2", "u2", d2_out);

        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);

        // u1's client may open a session to u1's device…
        assert!(state.open_session("s1", client, "dev-u1").is_some());
        // …but not to u2's device.
        assert!(state.open_session("s2", client, "dev-u2").is_none());
        // …and not to an unknown device.
        assert!(state.open_session("s3", client, "ghost").is_none());
    }

    #[test]
    fn session_frames_route_between_the_two_peers() {
        let mut state = RelayState::new();
        let (d_out, mut d_rx) = chan();
        let conn = state.add_device("dev", "u1", d_out).conn_id;
        let (c_out, mut c_rx) = chan();
        let client = state.add_client("u1", c_out);
        state.open_session("s1", client, "dev").unwrap();

        // client → device
        state
            .device_out_for_client_frame("s1", client)
            .unwrap()
            .send("to-device".into())
            .unwrap();
        assert_eq!(d_rx.try_recv().unwrap(), "to-device");

        // device → client
        assert_eq!(
            state.route_device_frame("s1", "dev", conn, "to-client".into()),
            DeviceFrameRoute::Forwarded
        );
        assert_eq!(c_rx.try_recv().unwrap(), "to-client");
    }

    #[test]
    fn another_clients_session_is_not_routable() {
        let mut state = RelayState::new();
        let (d_out, _d_rx) = chan();
        state.add_device("dev", "u1", d_out);
        let (c1_out, _c1_rx) = chan();
        let (c2_out, _c2_rx) = chan();
        let c1 = state.add_client("u1", c1_out);
        let c2 = state.add_client("u1", c2_out);
        state.open_session("s1", c1, "dev").unwrap();
        // c2 cannot drive c1's session.
        assert!(state.device_out_for_client_frame("s1", c2).is_none());
    }

    #[test]
    fn transport_key_fanout_targets_only_owner_clients() {
        let mut state = RelayState::new();
        let (d_out, _d_rx) = chan();
        let conn = state.add_device("dev", "u1", d_out).conn_id;
        let (c1_out, _c1) = chan();
        let (c2_out, _c2) = chan();
        state.add_client("u1", c1_out);
        state.add_client("u2", c2_out);
        let target_count = state
            .publish_device_transport_key("dev", conn, "KEY")
            .expect("current connection");
        assert_eq!(target_count, 1, "only u1's client is notified");
        assert_eq!(
            state.device_keys_for_user("u1"),
            vec![("dev".to_string(), "KEY".to_string())]
        );
        assert!(state.device_keys_for_user("u2").is_empty());
    }

    #[test]
    fn removing_a_device_drops_its_sessions() {
        let mut state = RelayState::new();
        let (d_out, _d_rx) = chan();
        let conn = state.add_device("dev", "u1", d_out).conn_id;
        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);
        state.open_session("s1", client, "dev").unwrap();
        state.remove_device("dev", conn);
        assert!(state.device_out_for_client_frame("s1", client).is_none());
    }

    #[test]
    fn removing_a_device_returns_only_owner_clients_to_notify() {
        let mut state = RelayState::new();
        let (d_out, _d_rx) = chan();
        let conn = state.add_device("dev", "u1", d_out).conn_id;
        let (c1_out, mut c1_rx) = chan();
        let (c2_out, mut c2_rx) = chan();
        state.add_client("u1", c1_out);
        state.add_client("u2", c2_out);

        let notify = state.remove_device("dev", conn).expect("really removed");
        assert_eq!(notify.len(), 1, "only the owner's client is notified");
        notify[0].send("device_offline".into()).unwrap();
        assert_eq!(c1_rx.try_recv().unwrap(), "device_offline");
        assert!(c2_rx.try_recv().is_err(), "other users hear nothing");
    }

    #[test]
    fn removing_an_unknown_device_is_a_stale_no_op() {
        let mut state = RelayState::new();
        let (c_out, _c_rx) = chan();
        state.add_client("u1", c_out);
        assert!(state.remove_device("ghost", 0).is_none());
    }

    #[test]
    fn stale_disconnect_does_not_remove_a_reconnected_device() {
        // A device reconnects before its old socket's cleanup runs: the stale
        // cleanup must not deregister the healthy new connection or notify anyone.
        let mut state = RelayState::new();
        let (old_out, _old_rx) = chan();
        let old_conn = state.add_device("dev", "u1", old_out).conn_id;
        let (new_out, mut new_rx) = chan();
        let new_conn = state.add_device("dev", "u1", new_out).conn_id; // reconnect, replaces entry
        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);

        // Old connection's cleanup fires late: must be a no-op the caller can tell
        // apart from a real removal — the device must NOT be reported offline.
        assert!(state.remove_device("dev", old_conn).is_none());
        assert!(
            state.open_session("s1", client, "dev").is_some(),
            "the reconnected device is still registered and routable"
        );
        state
            .device_out_for_client_frame("s1", client)
            .unwrap()
            .send("hi".into())
            .unwrap();
        assert_eq!(new_rx.try_recv().unwrap(), "hi");

        // The new connection's own cleanup still works.
        assert_eq!(
            state.remove_device("dev", new_conn).expect("removed").len(),
            1
        );
        assert!(state.open_session("s2", client, "dev").is_none());
    }

    #[test]
    fn a_client_cannot_hijack_another_clients_session_id() {
        let mut state = RelayState::new();
        let (d_out, mut d_rx) = chan();
        state.add_device("dev", "u1", d_out);
        let (c1_out, _c1_rx) = chan();
        let (c2_out, _c2_rx) = chan();
        let victim = state.add_client("u1", c1_out);
        let hijacker = state.add_client("u1", c2_out);
        state.open_session("s1", victim, "dev").unwrap();

        assert!(
            state.open_session("s1", hijacker, "dev").is_none(),
            "an existing session id belonging to another client is rejected"
        );
        // The victim's session still routes both ways.
        state
            .device_out_for_client_frame("s1", victim)
            .unwrap()
            .send("still-mine".into())
            .unwrap();
        assert_eq!(d_rx.try_recv().unwrap(), "still-mine");
        // The same client MAY re-init its own session id (a re-handshake).
        assert!(state.open_session("s1", victim, "dev").is_some());
    }

    #[test]
    fn client_disconnect_reports_severed_sessions_for_device_teardown() {
        let mut state = RelayState::new();
        let (d_out, mut d_rx) = chan();
        state.add_device("dev", "u1", d_out);
        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);
        state.open_session("s1", client, "dev").unwrap();
        state.open_session("s2", client, "dev").unwrap();

        let mut severed = state.remove_client(client);
        severed.sort_by(|a, b| a.0.cmp(&b.0));
        let ids: Vec<&str> = severed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2"], "each severed session is reported");
        severed[0].1.send("session_closed:s1".into()).unwrap();
        assert_eq!(d_rx.try_recv().unwrap(), "session_closed:s1");
    }

    #[test]
    fn client_disconnect_with_no_sessions_reports_nothing() {
        let mut state = RelayState::new();
        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);
        assert!(state.remove_client(client).is_empty());
    }

    #[test]
    fn relay_config_reads_production_env() {
        let vars: HashMap<&str, &str> = HashMap::from([
            ("RELAY_PORT", "9000"),
            ("API_INTERNAL_URL", "http://api.8ly.svc:8080/"),
            ("RELAY_INTERNAL_SECRET", "s3cret"),
            ("RELAY_DEVICE_LIVENESS_S", "120"),
            ("RELAY_WRITE_STALL_S", "10"),
        ]);
        let config = RelayConfig::from_lookup(|k| vars.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(config.port, 9000);
        assert_eq!(config.api_url, "http://api.8ly.svc:8080");
        assert_eq!(config.internal_secret.as_deref(), Some("s3cret"));
        assert_eq!(config.device_liveness_timeout, Duration::from_secs(120));
        assert_eq!(config.write_stall_timeout, Duration::from_secs(10));
    }

    #[test]
    fn relay_config_defaults_for_dev() {
        let config = RelayConfig::from_lookup(|_| None).unwrap();
        assert_eq!(config.port, 8799);
        assert_eq!(config.api_url, "http://127.0.0.1:8080");
        assert_eq!(config.internal_secret, None);
        assert_eq!(
            config.device_liveness_timeout,
            Duration::from_secs(3 * HEARTBEAT_INTERVAL_S)
        );
        assert_eq!(config.write_stall_timeout, Duration::from_secs(30));
    }

    #[test]
    fn device_liveness_default_tolerates_missed_heartbeats() {
        // The device heartbeats every HEARTBEAT_INTERVAL_S; the liveness deadline
        // must allow at least two missed beats, or a single dropped frame on a
        // healthy connection severs the device.
        let config = RelayConfig::from_lookup(|_| None).unwrap();
        assert!(config.device_liveness_timeout >= Duration::from_secs(2 * HEARTBEAT_INTERVAL_S));
    }

    #[test]
    fn relay_config_rejects_unparseable_timeouts() {
        let bad_liveness: HashMap<&str, &str> =
            HashMap::from([("RELAY_DEVICE_LIVENESS_S", "soon")]);
        assert!(RelayConfig::from_lookup(|k| bad_liveness.get(k).map(|v| v.to_string())).is_err());

        let zero_liveness: HashMap<&str, &str> = HashMap::from([("RELAY_DEVICE_LIVENESS_S", "0")]);
        assert!(
            RelayConfig::from_lookup(|k| zero_liveness.get(k).map(|v| v.to_string())).is_err(),
            "a zero liveness window would sever every device instantly"
        );

        let bad_stall: HashMap<&str, &str> = HashMap::from([("RELAY_WRITE_STALL_S", "0")]);
        assert!(RelayConfig::from_lookup(|k| bad_stall.get(k).map(|v| v.to_string())).is_err());
    }

    #[test]
    fn relay_config_falls_back_to_legacy_api_url_var() {
        let vars: HashMap<&str, &str> = HashMap::from([("RELAY_API_URL", "http://legacy:1234")]);
        let config = RelayConfig::from_lookup(|k| vars.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(config.api_url, "http://legacy:1234");
    }

    #[test]
    fn relay_config_rejects_unparseable_port_and_blank_secret() {
        let bad_port: HashMap<&str, &str> = HashMap::from([("RELAY_PORT", "not-a-port")]);
        assert!(RelayConfig::from_lookup(|k| bad_port.get(k).map(|v| v.to_string())).is_err());

        let blank: HashMap<&str, &str> = HashMap::from([("RELAY_INTERNAL_SECRET", "")]);
        let config = RelayConfig::from_lookup(|k| blank.get(k).map(|v| v.to_string())).unwrap();
        assert_eq!(config.internal_secret, None, "blank secret means unset");
    }

    #[test]
    fn health_request_detection() {
        assert!(is_health_request(b"GET /health HTTP/1.1\r\n"));
        assert!(is_health_request(b"GET /health?probe=1 HTTP/1.1\r\n"));
        assert!(!is_health_request(b"GET /healthz HTTP/1.1\r\n"));
        assert!(!is_health_request(b"GET /ws/client HTTP/1.1\r\n"));
        assert!(!is_health_request(b"POST /health HTTP/1.1\r\n"));
        assert!(!is_health_request(b"GET /heal"), "incomplete prefix");
    }

    #[test]
    fn parse_authenticate_token_accepts_only_the_documented_frame() {
        assert_eq!(
            parse_authenticate_token(r#"{"type":"authenticate","token":"gw_abc"}"#),
            Some("gw_abc".to_string())
        );
        assert_eq!(parse_authenticate_token(r#"{"type":"hello"}"#), None);
        assert_eq!(parse_authenticate_token(r#"{"type":"authenticate"}"#), None);
        assert_eq!(parse_authenticate_token("not json"), None);
    }

    #[test]
    fn session_target_device_prefers_route_to() {
        let msg: Value = serde_json::from_str(
            r#"{"type":"session_init","route_to":"device:dev-9","session_init":{"device_id":"dev-1"}}"#,
        )
        .unwrap();
        assert_eq!(session_target_device(&msg), Some("dev-9".to_string()));
    }

    #[test]
    fn session_target_device_falls_back_to_session_init_payload() {
        let msg: Value =
            serde_json::from_str(r#"{"type":"session_init","session_init":{"device_id":"dev-1"}}"#)
                .unwrap();
        assert_eq!(session_target_device(&msg), Some("dev-1".to_string()));
    }

    #[test]
    fn session_target_device_rejects_malformed_route_to() {
        let msg: Value = serde_json::from_str(
            r#"{"type":"session_init","route_to":"broadcast","session_init":{"device_id":"dev-1"}}"#,
        )
        .unwrap();
        assert_eq!(
            session_target_device(&msg),
            None,
            "a present-but-malformed route_to must not silently fall back"
        );
    }

    #[test]
    fn adding_a_device_reports_owner_clients_for_device_online_push() {
        let mut state = RelayState::new();
        let (owner_out, mut owner_rx) = chan();
        let (other_out, mut other_rx) = chan();
        state.add_client("u1", owner_out);
        state.add_client("u2", other_out);

        let (d_out, _d_rx) = chan();
        let registration = state.add_device("dev", "u1", d_out);
        assert_eq!(registration.owner_clients.len(), 1);
        registration.owner_clients[0]
            .send("device_online".into())
            .unwrap();
        assert_eq!(owner_rx.try_recv().unwrap(), "device_online");
        assert!(other_rx.try_recv().is_err(), "other users hear nothing");
    }

    #[test]
    fn one_client_holds_sessions_to_multiple_owned_devices_concurrently() {
        let mut state = RelayState::new();
        let (d1_out, mut d1_rx) = chan();
        let (d2_out, mut d2_rx) = chan();
        let conn_1 = state.add_device("dev-1", "u1", d1_out).conn_id;
        let conn_2 = state.add_device("dev-2", "u1", d2_out).conn_id;
        let (c_out, mut c_rx) = chan();
        let client = state.add_client("u1", c_out);

        assert!(state.open_session("s1", client, "dev-1").is_some());
        assert!(state.open_session("s2", client, "dev-2").is_some());

        // Frames route independently per session.
        state
            .device_out_for_client_frame("s1", client)
            .unwrap()
            .send("to-dev-1".into())
            .unwrap();
        state
            .device_out_for_client_frame("s2", client)
            .unwrap()
            .send("to-dev-2".into())
            .unwrap();
        assert_eq!(d1_rx.try_recv().unwrap(), "to-dev-1");
        assert!(d1_rx.try_recv().is_err(), "dev-1 sees only its session");
        assert_eq!(d2_rx.try_recv().unwrap(), "to-dev-2");

        // Replies come back on the right sessions too.
        assert_eq!(
            state.route_device_frame("s1", "dev-1", conn_1, "from-dev-1".into()),
            DeviceFrameRoute::Forwarded
        );
        assert_eq!(c_rx.try_recv().unwrap(), "from-dev-1");
        // A device cannot answer on the other device's session.
        assert_eq!(
            state.route_device_frame("s1", "dev-2", conn_2, "wrong-device".into()),
            DeviceFrameRoute::NoRoute
        );
    }

    #[test]
    fn cross_user_session_is_rejected_even_with_valid_route() {
        let mut state = RelayState::new();
        let (d_out, mut d_rx) = chan();
        state.add_device("dev-victim", "victim", d_out);
        let (c_out, _c_rx) = chan();
        let attacker = state.add_client("attacker", c_out);

        assert!(
            state.open_session("s1", attacker, "dev-victim").is_none(),
            "ownership is enforced on session_init"
        );
        assert!(
            state.device_out_for_client_frame("s1", attacker).is_none(),
            "no session state leaked from the rejected attempt"
        );
        assert!(d_rx.try_recv().is_err(), "the device never hears about it");
    }

    #[test]
    fn reconnect_severs_stale_sessions_and_reports_their_clients() {
        // A device blips and reconnects before the old socket's cleanup: the old
        // sessions' keys died with the old process, so the clients riding them must
        // be reported for a re-handshake nudge — and the dead sessions must not
        // route frames to the new connection.
        let mut state = RelayState::new();
        let (old_out, _old_rx) = chan();
        state.add_device("dev", "u1", old_out);
        let (c_out, _c_rx) = chan();
        let client = state.add_client("u1", c_out);
        state.open_session("s1", client, "dev").unwrap();
        let (bystander_out, mut bystander_rx) = chan();
        state.add_client("u1", bystander_out); // connected, but no session

        let (new_out, _new_rx) = chan();
        let reg = state.add_device("dev", "u1", new_out);
        assert_eq!(reg.displaced_clients.len(), 1, "only the session's client");
        reg.displaced_clients[0]
            .send("re-handshake".into())
            .unwrap();
        assert!(bystander_rx.try_recv().is_err(), "bystander not nudged");
        assert!(
            state.device_out_for_client_frame("s1", client).is_none(),
            "the dead session no longer routes"
        );
    }

    #[test]
    fn reconnect_signals_the_superseded_connection() {
        let mut state = RelayState::new();
        let (old_out, _old_rx) = chan();
        let mut old = state.add_device("dev", "u1", old_out);
        let (new_out, _new_rx) = chan();
        let new = state.add_device("dev", "u1", new_out);

        assert_eq!(new.superseded_conn_id, Some(old.conn_id));
        assert_eq!(old.superseded.try_recv(), Ok(()));
        assert!(state.remove_device("dev", old.conn_id).is_none());
        assert!(state.device_connection_is_current("dev", new.conn_id));
    }

    #[test]
    fn stale_connection_cannot_mutate_keys_or_route_frames() {
        let mut state = RelayState::new();
        let (old_out, _old_rx) = chan();
        let old_conn = state.add_device("dev", "u1", old_out).conn_id;
        let (client_out, mut client_rx) = chan();
        let client = state.add_client("u1", client_out);
        state.open_session("reused", client, "dev").unwrap();

        let (new_out, _new_rx) = chan();
        let new_conn = state.add_device("dev", "u1", new_out).conn_id;
        state.open_session("reused", client, "dev").unwrap();

        assert!(state
            .publish_device_transport_key("dev", old_conn, "STALE")
            .is_none());
        assert!(state.device_keys_for_user("u1").is_empty());
        assert_eq!(
            state.route_device_frame("reused", "dev", old_conn, "stale".into()),
            DeviceFrameRoute::StaleConnection
        );
        assert!(client_rx.try_recv().is_err());

        state
            .publish_device_transport_key("dev", new_conn, "CURRENT")
            .expect("new connection owns the key");
        assert_eq!(
            state.route_device_frame("reused", "dev", new_conn, "current".into()),
            DeviceFrameRoute::Forwarded
        );
        assert_eq!(
            state.device_keys_for_user("u1"),
            vec![("dev".to_string(), "CURRENT".to_string())]
        );
    }
}
