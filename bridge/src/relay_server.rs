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
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::transport;

/// The signed-challenge path (`{ts}.GET./ws/device`) the device proves.
pub const AUTH_PATH: &str = "/ws/device";
/// Allowed clock skew between the device's timestamp and the relay.
pub const AUTH_SKEW: Duration = Duration::from_secs(60);

/// A connected peer's outbound channel. Text payloads only; the bin wraps them in
/// WebSocket frames. Decoupled from tungstenite so this module stays testable.
pub type Outbound = mpsc::UnboundedSender<String>;

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
}

struct ConnectedClient {
    user_id: String,
    out: Outbound,
}

struct Session {
    client_id: u64,
    device_id: String,
}

/// The result of registering a device connection: its connection id, and any clients
/// whose sessions were severed because this is a reconnect of the same device.
pub struct DeviceRegistration {
    pub conn_id: u64,
    pub displaced_clients: Vec<Outbound>,
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

        self.devices.insert(
            device_id.to_string(),
            ConnectedDevice {
                owner_user_id: owner_user_id.to_string(),
                transport_key: None,
                out,
                conn_id,
            },
        );
        DeviceRegistration {
            conn_id,
            displaced_clients,
        }
    }

    /// Record a device's transport key and return the outbounds of every connected
    /// client owned by the same user, so the bin can push `device_key` to them.
    pub fn set_device_transport_key(&mut self, device_id: &str, key: &str) -> Vec<Outbound> {
        let owner = match self.devices.get_mut(device_id) {
            Some(device) => {
                device.transport_key = Some(key.to_string());
                device.owner_user_id.clone()
            }
            None => return Vec::new(),
        };
        self.client_outbounds_for_user(&owner)
    }

    /// Remove a device and drop any sessions routed to it — but only if the entry
    /// still belongs to `conn_id`, so a stale socket's late cleanup is a no-op after
    /// the device reconnected. Returns the outbounds of the owner's connected clients
    /// so the bin can push a `device_offline` notice — the browser's cue to degrade
    /// gracefully instead of hanging on a dead session.
    pub fn remove_device(&mut self, device_id: &str, conn_id: u64) -> Vec<Outbound> {
        match self.devices.get(device_id) {
            Some(device) if device.conn_id == conn_id => {}
            _ => return Vec::new(),
        }
        let device = self.devices.remove(device_id).expect("checked above");
        self.sessions.retain(|_, s| s.device_id != device_id);
        self.client_outbounds_for_user(&device.owner_user_id)
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

    /// Remove a client and drop its sessions.
    pub fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
        self.sessions.retain(|_, s| s.client_id != client_id);
    }

    // ----- routing -----------------------------------------------------------

    /// Open a session from a client to a device, enforcing ownership. Returns the
    /// device's outbound to forward the `session_init` to, or `None` if the device is
    /// unknown or owned by a different user (the cross-account guard).
    pub fn open_session(
        &mut self,
        session_id: &str,
        client_id: u64,
        device_id: &str,
    ) -> Option<Outbound> {
        let user_id = self.clients.get(&client_id)?.user_id.clone();
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

    /// The client outbound for a device's in-session frame — only if the session belongs
    /// to that device.
    pub fn client_out_for_device_frame(
        &self,
        session_id: &str,
        device_id: &str,
    ) -> Option<Outbound> {
        let session = self.sessions.get(session_id)?;
        if session.device_id != device_id {
            return None;
        }
        self.clients.get(&session.client_id).map(|c| c.out.clone())
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
    let mut peeked = [0u8; HEALTH_REQUEST_PREFIX.len()];
    loop {
        let n = tcp.peek(&mut peeked).await?;
        if n == 0 {
            return Ok(false); // peer closed before sending a request line
        }
        if peeked[..n] != HEALTH_REQUEST_PREFIX[..n] {
            return Ok(false); // diverges from `GET /health` → not a probe
        }
        if n == peeked.len() {
            break; // full prefix seen: this is a probe
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

    fn chan() -> (Outbound, mpsc::UnboundedReceiver<String>) {
        mpsc::unbounded_channel()
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
        state.add_device("dev", "u1", d_out);
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
        state
            .client_out_for_device_frame("s1", "dev")
            .unwrap()
            .send("to-client".into())
            .unwrap();
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
        state.add_device("dev", "u1", d_out);
        let (c1_out, _c1) = chan();
        let (c2_out, _c2) = chan();
        state.add_client("u1", c1_out);
        state.add_client("u2", c2_out);
        let targets = state.set_device_transport_key("dev", "KEY");
        assert_eq!(targets.len(), 1, "only u1's client is notified");
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

        let notify = state.remove_device("dev", conn);
        assert_eq!(notify.len(), 1, "only the owner's client is notified");
        notify[0].send("device_offline".into()).unwrap();
        assert_eq!(c1_rx.try_recv().unwrap(), "device_offline");
        assert!(c2_rx.try_recv().is_err(), "other users hear nothing");
    }

    #[test]
    fn removing_an_unknown_device_notifies_nobody() {
        let mut state = RelayState::new();
        let (c_out, _c_rx) = chan();
        state.add_client("u1", c_out);
        assert!(state.remove_device("ghost", 0).is_empty());
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

        // Old connection's cleanup fires late: must be a no-op.
        assert!(state.remove_device("dev", old_conn).is_empty());
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
        assert_eq!(state.remove_device("dev", new_conn).len(), 1);
        assert!(state.open_session("s2", client, "dev").is_none());
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
}
