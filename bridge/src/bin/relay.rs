//! The relay broker: forwards opaque E2EE envelopes between devices (`/ws/device`)
//! and browser clients (`/ws/client`), authenticating both sides and scoping all
//! routing per user.
//!
//! **TLS**: this process terminates **plain WebSocket only** — by design. In
//! production it runs behind traefik, which terminates TLS/`wss://` at the ingress
//! and forwards plain `ws://` in-cluster; there is deliberately no TLS configuration // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
//! here. Never expose the relay's port directly to the internet.
//!
//! - `/ws/device`: verifies the Ed25519-signed upgrade challenge against the device's
//!   record at the api (`GET /internal/devices/{id}`); admits only approved devices,
//!   inside the clock-skew window, not replayed.
//! - `/ws/client`: the browser's **first frame** must be
//!   `{"type":"authenticate","token":...}` with a gateway token the api minted
//!   (`GET /internal/gateway-token/{token}`); anything else disconnects — an
//!   unauthenticated client gets exactly one frame.
//! - `GET /health`: plain HTTP 200 for k8s probes, no auth.
//!
//! Every internal api call carries `X-Internal-Secret` (from `RELAY_INTERNAL_SECRET`),
//! and an unreachable api **fails closed**: nobody authenticates. The relay never
//! decrypts anything — the auth/ownership decisions live in
//! [`build_bridge::relay_server`]; this bin is sockets + api lookups + shutdown.
//!
//! Config (see [`RelayConfig`]): `RELAY_PORT` (default 8799; `0` binds an ephemeral
//! port, printed in the banner), `API_INTERNAL_URL` (fallback: legacy `RELAY_API_URL`,
//! then `http://127.0.0.1:8080`), `RELAY_INTERNAL_SECRET`.
//!
//! On SIGTERM/SIGINT the relay stops accepting, tells every connection task to wind
//! down, and each socket is closed with a proper WS Close frame within a bounded
//! grace period.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use build_bridge::relay_server::{
    self, AuthOutcome, DeviceAuth, DeviceFrameRoute, DeviceRecord, Outbound, RelayConfig,
    RelayState, ReplayGuard, AUTH_SKEW, HEARTBEAT_INTERVAL_S, MAX_WS_MESSAGE_BYTES, REPLAY_TTL,
};

/// How long shutdown waits for connection tasks to say goodbye before exiting anyway.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// The whole pre-auth phase (health-probe peek + WS upgrade) must finish within
/// this, or a silent/slow-loris peer pins a task and an FD forever and blocks
/// graceful shutdown for the full grace period.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// An accepted client must send its `authenticate` frame within this.
const CLIENT_AUTH_DEADLINE: Duration = Duration::from_secs(10);
/// How often a connected device's approval is re-checked at the api, so revoking
/// a device actually severs its live relay connection (not just future ones).
const DEVICE_REVALIDATION_INTERVAL: Duration = Duration::from_secs(60);
/// Bound lifecycle status writes so a stuck api cannot prevent a replacement
/// connection from taking ownership of the device indefinitely.
const STATUS_REPORT_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound periodic authorization checks so supersession and shutdown cannot be
/// hidden behind a hung internal api request indefinitely.
const AUTHORIZATION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

struct Shared {
    state: Mutex<RelayState>,
    device_lifecycles: StdMutex<HashMap<String, Weak<Mutex<()>>>>,
    replay: Mutex<ReplayGuard>,
    http: reqwest::Client,
    config: RelayConfig,
}

impl Shared {
    fn device_lifecycle(&self, device_id: &str) -> Arc<Mutex<()>> {
        let mut lifecycles = self
            .device_lifecycles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lifecycles.retain(|_, lifecycle| lifecycle.strong_count() > 0);
        if let Some(lifecycle) = lifecycles.get(device_id).and_then(Weak::upgrade) {
            return lifecycle;
        }
        let lifecycle = Arc::new(Mutex::new(()));
        lifecycles.insert(device_id.to_string(), Arc::downgrade(&lifecycle));
        lifecycle
    }

    /// A GET to an internal api endpoint, carrying `X-Internal-Secret` when configured.
    fn internal_get(&self, url: &str) -> reqwest::RequestBuilder {
        self.attach_internal_secret(self.http.get(url))
    }

    /// A POST to an internal api endpoint, carrying `X-Internal-Secret` when configured.
    fn internal_post(&self, url: &str) -> reqwest::RequestBuilder {
        self.attach_internal_secret(self.http.post(url))
    }

    fn attach_internal_secret(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.config.internal_secret {
            Some(secret) => builder.header("X-Internal-Secret", secret),
            None => builder, // dev mode: the api trusts localhost instead
        }
    }
}

/// What the WS upgrade callback captured before we accepted the socket.
#[derive(Default, Clone)]
struct Upgrade {
    path: String,
    device_id: Option<String>,
    timestamp: Option<String>,
    signature: Option<String>,
}

#[tokio::main]
async fn main() {
    let config = match RelayConfig::from_lookup(|name| std::env::var(name).ok()) {
        Ok(config) => config,
        Err(why) => {
            eprintln!("relay: config error: {why}");
            std::process::exit(2);
        }
    };
    let listener = TcpListener::bind(("0.0.0.0", config.port))
        .await
        .expect("relay port binds");
    let bound_port = listener.local_addr().expect("bound address").port();
    let shared = Arc::new(Shared {
        state: Mutex::new(RelayState::new()),
        device_lifecycles: StdMutex::new(HashMap::new()),
        replay: Mutex::new(ReplayGuard::new(REPLAY_TTL)),
        http: reqwest::Client::new(),
        config,
    });

    // Plain ws by design: traefik terminates TLS at the ingress (see module docs).
    println!(
        "RELAY_LISTENING ws://0.0.0.0:{bound_port}  (device→/ws/device [authed], client→/ws/client [token], GET /health)" // nosemgrep
    );

    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let mut connections = tokio::task::JoinSet::new();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler installs");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (tcp, _) = match accepted {
                    Ok(connection) => connection,
                    Err(e) => {
                        eprintln!("relay: accept error: {e}");
                        continue;
                    }
                };
                let shared = Arc::clone(&shared);
                let shutdown = shutdown_tx.subscribe();
                connections.spawn(async move {
                    if let Err(e) = serve(tcp, shared, shutdown).await {
                        eprintln!("relay: connection error: {e}");
                    }
                });
            }
            _ = sigterm.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    // Graceful shutdown: stop accepting, tell every connection task to wind down
    // (each says goodbye with a WS Close frame), then exit within the grace period.
    eprintln!(
        "relay: shutting down, closing {} connection(s)",
        connections.len()
    );
    drop(listener);
    let _ = shutdown_tx.send(());
    let drain_all = async { while connections.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, drain_all)
        .await
        .is_err()
    {
        eprintln!("relay: shutdown grace period elapsed with connections still open");
    }
}

// The tungstenite accept callback's error type is large and fixed by the API.
#[allow(clippy::result_large_err)]
async fn serve(
    mut tcp: TcpStream,
    shared: Arc<Shared>,
    mut shutdown: broadcast::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Capture the path + device auth headers during the handshake. Structurally
    // invalid device upgrades (missing headers) are rejected here with a non-101.
    let captured = Arc::new(std::sync::Mutex::new(Upgrade::default()));
    let capture = Arc::clone(&captured);

    // The whole pre-auth phase — health-probe peek + WS upgrade — runs under one
    // deadline and honors shutdown, so a silent peer can neither pin this task
    // forever nor stall SIGTERM for the full grace period.
    let pre_auth = async move {
        // Plain-HTTP `GET /health` (Kubernetes liveness/readiness) is answered before
        // the WebSocket handshake; the check peeks, so upgrades pass through untouched.
        if relay_server::handle_health_probe(&mut tcp).await? {
            return Ok::<_, Box<dyn std::error::Error>>(None);
        }
        let ws = tokio_tungstenite::accept_hdr_async_with_config(
            tcp,
            move |req: &Request, resp: Response| {
                let path = req.uri().path().to_string();
                let header = |name: &str| {
                    req.headers()
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                };
                let upgrade = Upgrade {
                    device_id: header("x-device-id"),
                    timestamp: header("x-timestamp"),
                    signature: header("x-signature"),
                    path: path.clone(),
                };
                if path == "/ws/device"
                    && (upgrade.device_id.is_none()
                        || upgrade.timestamp.is_none()
                        || upgrade.signature.is_none())
                {
                    let mut err = ErrorResponse::new(Some("missing device auth headers".into()));
                    *err.status_mut() = StatusCode::UNAUTHORIZED;
                    return Err(err);
                }
                *capture.lock().unwrap() = upgrade;
                Ok(resp)
            },
            Some(websocket_limits()),
        )
        .await?;
        Ok(Some(ws))
    };
    let ws = tokio::select! {
        outcome = tokio::time::timeout(HANDSHAKE_DEADLINE, pre_auth) => match outcome {
            Ok(Ok(Some(ws))) => ws,
            Ok(Ok(None)) => return Ok(()), // health probe: answered and done
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(()), // slow-loris / idle socket: drop it
        },
        _ = shutdown.recv() => return Ok(()),
    };
    let upgrade = captured.lock().unwrap().clone();

    let (mut sink, mut source) = ws.split();
    // One writer owns the sink; peers queue text payloads to it through a
    // byte-bounded channel, and a stalled TCP write severs the peer — one slow
    // reader can never grow relay memory without limit.
    //
    // The writer's death must tear down the WHOLE connection, not just the sink:
    // `writer_gone` completes when the writer task exits abnormally, and the serve
    // loops treat that as a disconnect. Without it a stalled write left the peer
    // registered with a dead outbound — every frame routed to it silently dropped
    // while browsers were told the device was still online.
    let write_stall_timeout = shared.config.write_stall_timeout;
    // Ping often enough that a responsive peer refreshes its pong deadline
    // several times per liveness window — one dropped ping must not sever it.
    let ping_interval = shared.config.device_liveness_timeout / 3;
    let (out_tx, mut out_rx) = relay_server::outbound_channel();
    let (writer_gone_tx, mut writer_gone) = tokio::sync::oneshot::channel::<()>();
    let mut writer = tokio::spawn(async move {
        // Dropped (without send) on any exit path: severance and panic alike
        // resolve `writer_gone`, while the graceful path below sends first.
        let graceful = writer_gone_tx;
        // WebSocket pings ride the same sink. A pong comes back only when the
        // peer's READ loop polls its socket — which is exactly what a wedged
        // bridge stops doing while its heartbeat task keeps writing, and what a
        // browser that is gone never does again. Both serve loops enforce the
        // pong deadline: a browser pongs from the WS stack itself, so a ping it
        // does not answer is the one proof the relay gets that nobody is there
        // when a load balancer keeps the TCP connection established regardless.
        //
        // The first ping waits a full interval: a peer that just completed the
        // handshake has proven liveness, and pinging at spawn races the
        // `authenticated` greeting for the sink — the greeting must go first.
        let mut ping =
            tokio::time::interval_at(tokio::time::Instant::now() + ping_interval, ping_interval);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let message = tokio::select! {
                item = out_rx.recv() => match item {
                    Some(text) => Message::Text(text),
                    None => break,
                },
                _ = ping.tick() => Message::Ping(Vec::new()),
            };
            match tokio::time::timeout(write_stall_timeout, sink.send(message)).await {
                Ok(Ok(())) => {}
                _ => return, // peer gone, or it stopped reading: sever
            }
        }
        // Every sender is gone: this peer is being disconnected on purpose (cleanup
        // or shutdown) — say goodbye with a proper Close frame, not a dropped stream.
        let _ = graceful.send(());
        let _ = sink.send(Message::Close(None)).await;
    });

    if upgrade.path == "/ws/device" {
        serve_device(
            &shared,
            &upgrade,
            out_tx,
            &mut source,
            &mut shutdown,
            &mut writer_gone,
        )
        .await;
    } else {
        serve_client(
            &shared,
            out_tx,
            &mut source,
            &mut shutdown,
            &mut writer_gone,
        )
        .await;
    }

    // All out_tx clones die with the state cleanup above, which lets the writer
    // drain, send Close, and finish.
    if tokio::time::timeout(SHUTDOWN_GRACE, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
        let _ = writer.await;
    }
    Ok(())
}

/// Enforce sanity limits on every accepted socket: no message or frame larger than
/// [`MAX_WS_MESSAGE_BYTES`] is ever buffered.
fn websocket_limits() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_WS_MESSAGE_BYTES),
        max_frame_size: Some(MAX_WS_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

#[allow(clippy::cognitive_complexity)] // ratchet: serve_device is at 32, threshold 15 — bring it under, then remove
async fn serve_device(
    shared: &Arc<Shared>,
    upgrade: &Upgrade,
    out_tx: Outbound,
    source: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    shutdown: &mut broadcast::Receiver<()>,
    writer_gone: &mut tokio::sync::oneshot::Receiver<()>,
) {
    let (device_id, timestamp, signature) = match (
        upgrade.device_id.clone(),
        upgrade.timestamp.clone(),
        upgrade.signature.clone(),
    ) {
        (Some(d), Some(t), Some(s)) => (d, t, s),
        _ => return, // structurally rejected already; defensive.
    };

    // Look up the device record at the api. An unreachable api fails closed.
    let Some(record) = lookup_device(shared, &device_id).await else {
        eprintln!("device {device_id}: unknown to api or api unreachable; refused (fail closed)");
        return;
    };

    // Replay + signature/approval/skew checks.
    let now_unix = unix_now();
    {
        let mut replay = shared.replay.lock().await;
        if !replay.check_and_record(&device_id, &timestamp, &signature, Instant::now()) {
            eprintln!("device {device_id}: replayed challenge; refused");
            return;
        }
    }
    let auth = DeviceAuth {
        device_id: device_id.clone(),
        timestamp,
        signature,
    };
    let owner = match relay_server::authorize_device(&auth, &record, now_unix, AUTH_SKEW) {
        AuthOutcome::Ok { owner_user_id } => owner_user_id,
        AuthOutcome::Reject(why) => {
            eprintln!("device {device_id}: refused ({why})");
            return;
        }
    };
    let lifecycle = shared.device_lifecycle(&device_id);

    let mut registration = {
        let _transition = lifecycle.lock().await;
        let registration = {
            let mut state = shared.state.lock().await;
            state.add_device(&device_id, &owner, out_tx.clone())
        };
        if let Some(old_conn_id) = registration.superseded_conn_id {
            eprintln!(
                "device {device_id}: connection {} supersedes connection {old_conn_id}; closing old socket",
                registration.conn_id
            );
        }
        // A reconnect severed any sessions from this device's previous connection
        // (their keys died with the old process) — nudge those clients first.
        let stale_notice = json!({"type":"device_offline","device_id":device_id}).to_string();
        for client in &registration.displaced_clients {
            let _ = client.send(stale_notice.clone());
        }
        let online_notice = json!({"type":"device_online","device_id":device_id}).to_string();
        for client in &registration.owner_clients {
            let _ = client.send(online_notice.clone());
        }
        let _ = out_tx.send(
            json!({"type":"authenticated","device_id":device_id,"heartbeat_interval_s":HEARTBEAT_INTERVAL_S}).to_string(),
        );
        report_status(shared, &device_id, true).await;
        eprintln!(
            "device {device_id}: authenticated (owner {owner}, connection {})",
            registration.conn_id
        );
        registration
    };

    // Auth happens once at connect, so revocation must be re-checked while the
    // connection lives — otherwise "Revoke" in the app never cuts off a
    // compromised device until it happens to reconnect.
    let mut revalidate = tokio::time::interval_at(
        tokio::time::Instant::now() + DEVICE_REVALIDATION_INTERVAL,
        DEVICE_REVALIDATION_INTERVAL,
    );
    revalidate.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // The device promised a heartbeat every HEARTBEAT_INTERVAL_S, and its read
    // loop answers the writer's WebSocket pings with pongs. Hold it to BOTH:
    // text frames prove the device's send side, pongs prove its read loop. The
    // 2026-08-13 wedge sent heartbeats from a healthy task while the read loop
    // was stuck — frames alone said "alive" as the socket filled with unread
    // data and browsers hung on "Waiting for your device". Whichever signal
    // goes silent past the window severs the device, which also gets it
    // deregistered and reported offline below.
    let liveness_timeout = shared.config.device_liveness_timeout;
    let mut frame_deadline = tokio::time::Instant::now() + liveness_timeout;
    let mut pong_deadline = tokio::time::Instant::now() + liveness_timeout;

    'device: loop {
        let message = tokio::select! {
            biased;
            _ = &mut registration.superseded => {
                eprintln!(
                    "device {device_id}: connection {} superseded; ending old socket",
                    registration.conn_id
                );
                return;
            }
            _ = revalidate.tick() => {
                // Fail open only on an unreachable api (a blip must not drop every
                // device); an affirmative "not approved / unknown" severs now.
                let authorization = tokio::select! {
                    biased;
                    _ = &mut registration.superseded => {
                        eprintln!(
                            "device {device_id}: connection {} superseded during authorization check",
                            registration.conn_id
                        );
                        return;
                    }
                    _ = shutdown.recv() => break 'device,
                    result = tokio::time::timeout(
                        AUTHORIZATION_CHECK_TIMEOUT,
                        device_authorization(shared, &device_id, &owner),
                    ) => result,
                };
                if matches!(authorization, Ok(Some(false))) {
                    eprintln!("device {device_id}: no longer authorized; severing");
                    break;
                }
                continue;
            }
            _ = tokio::time::sleep_until(frame_deadline.min(pong_deadline)) => {
                let starved = if pong_deadline < frame_deadline { "pings unanswered (read loop dead)" } else { "no frames (send side dead)" };
                eprintln!(
                    "device {device_id}: {starved} for {}s; severing",
                    liveness_timeout.as_secs()
                );
                break;
            }
            _ = &mut *writer_gone => {
                eprintln!("device {device_id}: writer severed (stalled or failed write); severing");
                break;
            }
            _ = shutdown.recv() => break,
            next = source.next() => match next {
                Some(Ok(message)) => message,
                Some(Err(_)) => {
                    eprintln!(
                        "device {device_id}: connection {} read failed",
                        registration.conn_id
                    );
                    break;
                }
                None => {
                    eprintln!(
                        "device {device_id}: connection {} reached transport EOF",
                        registration.conn_id
                    );
                    break;
                }
            },
        };
        let Message::Text(text) = message else {
            if matches!(message, Message::Pong(_)) {
                pong_deadline = tokio::time::Instant::now() + liveness_timeout;
            }
            if matches!(message, Message::Close(_)) {
                eprintln!(
                    "device {device_id}: connection {} received peer close",
                    registration.conn_id
                );
                break;
            }
            continue;
        };
        frame_deadline = tokio::time::Instant::now() + liveness_timeout;
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match msg.get("type").and_then(Value::as_str).unwrap_or("") {
            "transport_key" => {
                let key = msg
                    .get("transport_public_key")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let client_count = {
                    let mut state = shared.state.lock().await;
                    state.publish_device_transport_key(&device_id, registration.conn_id, &key)
                };
                let Some(client_count) = client_count else {
                    eprintln!(
                        "device {device_id}: ignored transport key from stale connection {}",
                        registration.conn_id
                    );
                    continue;
                };
                eprintln!(
                    "device {device_id}: transport key → fan-out to {} client(s)",
                    client_count
                );
            }
            "heartbeat" => {}
            "session_accept" | "e2ee_envelope" => {
                if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                    let route = {
                        let mut state = shared.state.lock().await;
                        state.route_device_frame(session_id, &device_id, registration.conn_id, text)
                    };
                    if route == DeviceFrameRoute::StaleConnection {
                        eprintln!(
                            "device {device_id}: ignored session frame from stale connection {}",
                            registration.conn_id
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // The device is gone: drop it, then tell the owner's browsers immediately so
    // they can degrade gracefully (and reconnect on the next device_online) instead
    // of hanging on a dead session. Guarded by conn_id: if the device already
    // reconnected, this stale cleanup is a no-op — nobody is notified and the api
    // is NOT told the (live, routable) device went offline.
    let _transition = lifecycle.lock().await;
    let removal = {
        let mut state = shared.state.lock().await;
        state.remove_device(&device_id, registration.conn_id)
    };
    let Some(clients) = removal else {
        eprintln!(
            "device {device_id}: stale cleanup for connection {} ignored",
            registration.conn_id
        );
        return;
    };
    let notice = json!({"type":"device_offline","device_id":device_id}).to_string();
    for client in clients {
        let _ = client.send(notice.clone());
    }
    report_status(shared, &device_id, false).await;
}

#[allow(clippy::cognitive_complexity)] // ratchet: serve_client is at 26, threshold 15 — bring it under, then remove
async fn serve_client(
    shared: &Arc<Shared>,
    out_tx: Outbound,
    source: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    shutdown: &mut broadcast::Receiver<()>,
    writer_gone: &mut tokio::sync::oneshot::Receiver<()>,
) {
    // The browser gets exactly one frame while unauthenticated: it must be a text
    // `authenticate` frame carrying a valid gateway token — sent promptly — or the
    // connection ends (a silent socket must not pin this task forever).
    let first_frame = tokio::select! {
        next = tokio::time::timeout(CLIENT_AUTH_DEADLINE, source.next()) => match next {
            Ok(frame) => frame,
            Err(_) => {
                eprintln!("client: no authenticate frame within the deadline; refused");
                return;
            }
        },
        _ = shutdown.recv() => return,
    };
    let Some(Ok(Message::Text(first_text))) = first_frame else {
        eprintln!("client: first frame was not text; refused");
        return;
    };
    let Some(token) = relay_server::parse_authenticate_token(&first_text) else {
        eprintln!("client: first frame was not a valid authenticate; refused");
        return;
    };
    let Some(user_id) = lookup_gateway_token(shared, &token).await else {
        eprintln!("client: invalid gateway token or api unreachable; refused (fail closed)");
        return;
    };

    let client_id = {
        let mut state = shared.state.lock().await;
        state.add_client(&user_id, out_tx.clone())
    };
    eprintln!("client {client_id}: connected (user {user_id})");
    let _ = out_tx.send(json!({"type":"authenticated"}).to_string());
    // Advertise transport keys of the user's already-connected devices.
    let keys = {
        let state = shared.state.lock().await;
        state.device_keys_for_user(&user_id)
    };
    for (device_id, key) in keys {
        let _ = out_tx.send(
            json!({"type":"device_key","device_id":device_id,"transport_public_key":key})
                .to_string(),
        );
    }

    // A browser is held to the pong deadline alone: a quiet one sends no frame
    // for as long as it likes, but its WS stack answers every ping as long as
    // the page is there. Unanswered pings past the window are a client that
    // went away without a close — a suspended tab, a socket a load balancer
    // keeps established for a browser that is gone — and until it is severed,
    // its sessions pin the device's keys and its socket counts as a live client.
    let liveness_timeout = shared.config.device_liveness_timeout;
    let mut pong_deadline = tokio::time::Instant::now() + liveness_timeout;

    loop {
        let message = tokio::select! {
            next = source.next() => match next {
                Some(Ok(message)) => message,
                _ => break,
            },
            _ = tokio::time::sleep_until(pong_deadline) => {
                eprintln!(
                    "client {client_id}: pings unanswered for {}s; severing",
                    liveness_timeout.as_secs()
                );
                break;
            }
            _ = &mut *writer_gone => {
                eprintln!("client {client_id}: writer severed (stalled or failed write); severing");
                break;
            }
            _ = shutdown.recv() => break,
        };
        let Message::Text(text) = message else {
            if matches!(message, Message::Pong(_)) {
                pong_deadline = tokio::time::Instant::now() + liveness_timeout;
            }
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(session_id) = msg.get("session_id").and_then(Value::as_str) else {
            continue;
        };
        match msg.get("type").and_then(Value::as_str).unwrap_or("") {
            "session_init" => {
                // Contract: outer `route_to: "device:<id>"`; legacy fallback is the
                // device_id inside the session_init payload.
                let Some(device_id) = relay_server::session_target_device(&msg) else {
                    eprintln!("client {client_id}: session_init without a valid device target");
                    continue;
                };
                let target = {
                    let mut state = shared.state.lock().await;
                    state.open_session(session_id, client_id, &device_id)
                };
                match target {
                    Some(device) => {
                        let _ = device.send(text);
                    }
                    None => eprintln!("client {user_id}: rejected session to device {device_id}"),
                }
            }
            "e2ee_envelope" => {
                let target = {
                    let state = shared.state.lock().await;
                    state.device_out_for_client_frame(session_id, client_id)
                };
                if let Some(device) = target {
                    let _ = device.send(text);
                }
            }
            _ => {}
        }
    }

    eprintln!("client {client_id}: disconnected");
    // Tell each severed session's device the browser is gone, so it stops
    // encrypting terminal output into a session nobody will read and drops the
    // session key.
    let severed = {
        let mut state = shared.state.lock().await;
        state.remove_client(client_id)
    };
    for (session_id, device) in severed {
        let _ = device.send(json!({"type":"session_closed","session_id":session_id}).to_string());
    }
}

// --- api lookups (all fail closed: any error means "not authorized") -----------

async fn lookup_device(shared: &Arc<Shared>, device_id: &str) -> Option<DeviceRecord> {
    let url = format!("{}/internal/devices/{device_id}", shared.config.api_url);
    let resp = shared.internal_get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    Some(DeviceRecord {
        identity_public_key_b64: body.get("identity_public_key_b64")?.as_str()?.to_string(),
        approved: body
            .get("approved")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        owner_user_id: body
            .get("owner_user_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Re-check a connected device's authorization at the api.
/// `Some(true)` — still approved by the same owner; `Some(false)` — the api
/// affirmatively says revoked/unknown/re-owned (sever the connection);
/// `None` — the api is unreachable (fail open for an already-authenticated
/// connection: a blip must not drop every connected device).
async fn device_authorization(
    shared: &Arc<Shared>,
    device_id: &str,
    owner_user_id: &str,
) -> Option<bool> {
    let url = format!("{}/internal/devices/{device_id}", shared.config.api_url);
    let resp = shared.internal_get(&url).send().await.ok()?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Some(false);
    }
    if !resp.status().is_success() {
        return None; // 5xx etc.: treat like unreachable, keep the connection
    }
    let body: Value = resp.json().await.ok()?;
    let approved = body
        .get("approved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let same_owner = body.get("owner_user_id").and_then(Value::as_str) == Some(owner_user_id);
    Some(approved && same_owner)
}

async fn lookup_gateway_token(shared: &Arc<Shared>, token: &str) -> Option<String> {
    let url = format!("{}/internal/gateway-token/{token}", shared.config.api_url);
    let resp = shared.internal_get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    body.get("user_id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn report_status(shared: &Arc<Shared>, device_id: &str, online: bool) {
    let url = format!(
        "{}/internal/devices/{device_id}/status",
        shared.config.api_url
    );
    match tokio::time::timeout(
        STATUS_REPORT_TIMEOUT,
        shared
            .internal_post(&url)
            .json(&json!({ "online": online }))
            .send(),
    )
    .await
    {
        Err(_) => eprintln!("device {device_id}: status report timed out (online={online})"),
        Ok(Err(_)) => eprintln!("device {device_id}: status report failed (online={online})"),
        Ok(Ok(response)) if !response.status().is_success() => eprintln!(
            "device {device_id}: status report returned HTTP {} (online={online})",
            response.status().as_u16()
        ),
        Ok(Ok(_)) => {}
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
