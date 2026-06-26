//! A standalone dev relay: forwards opaque E2EE envelopes between devices
//! (`/ws/device`) and browser clients (`/ws/client`). Unlike the earlier global-pairing
//! stand-in, it now **authenticates** both sides and **scopes** routing per user:
//!
//! - `/ws/device`: verifies the Ed25519-signed upgrade challenge against the device's
//!   record at the api (`GET /internal/devices/{id}`); admits only approved devices,
//!   inside the clock-skew window, not replayed.
//! - `/ws/client`: the browser's first frame carries a gateway token the api minted;
//!   the relay validates it (`GET /internal/gateway-token/{token}`) to learn the user,
//!   then routes that browser only to devices the same user owns.
//!
//! It never decrypts anything — the auth/ownership decisions live in
//! [`build_bridge::relay_server`]; this bin is just sockets + api lookups.
//!
//! Config: `RELAY_PORT` (default 8799), `RELAY_API_URL` (default `http://127.0.0.1:8080`).

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;

use build_bridge::relay_server::{
    self, AuthOutcome, DeviceAuth, DeviceRecord, Outbound, RelayState, ReplayGuard, AUTH_SKEW,
};

struct Shared {
    state: Mutex<RelayState>,
    replay: Mutex<ReplayGuard>,
    http: reqwest::Client,
    api_url: String,
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
    let port: u16 = std::env::var("RELAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8799);
    let api_url =
        std::env::var("RELAY_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
    let shared = Arc::new(Shared {
        state: Mutex::new(RelayState::new()),
        replay: Mutex::new(ReplayGuard::new(AUTH_SKEW)),
        http: reqwest::Client::new(),
        api_url,
    });

    // The dev relay speaks plain (unencrypted) WebSocket by design — it is a local
    // stand-in; the production relay terminates TLS at the edge. This is only a banner.
    println!(
        "RELAY_LISTENING ws://0.0.0.0:{port}  (device→/ws/device [authed], client→/ws/client [token])" // nosemgrep
    );

    loop {
        let (tcp, _) = listener.accept().await.unwrap();
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = serve(tcp, shared).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

// The tungstenite accept callback's error type is large and fixed by the API.
#[allow(clippy::result_large_err)]
async fn serve(tcp: TcpStream, shared: Arc<Shared>) -> Result<(), Box<dyn std::error::Error>> {
    // Capture the path + device auth headers during the handshake. Structurally
    // invalid device upgrades (missing headers) are rejected here with a non-101.
    let captured = Arc::new(std::sync::Mutex::new(Upgrade::default()));
    let capture = Arc::clone(&captured);
    let ws = tokio_tungstenite::accept_hdr_async(tcp, move |req: &Request, resp: Response| {
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
    })
    .await?;
    let upgrade = captured.lock().unwrap().clone();

    let (mut sink, mut source) = ws.split();
    // One writer owns the sink; peers queue text payloads to it.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    if upgrade.path == "/ws/device" {
        serve_device(&shared, &upgrade, out_tx, &mut source).await;
    } else {
        serve_client(&shared, out_tx, &mut source).await;
    }

    writer.abort();
    Ok(())
}

async fn serve_device(
    shared: &Arc<Shared>,
    upgrade: &Upgrade,
    out_tx: Outbound,
    source: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) {
    let (device_id, timestamp, signature) = match (
        upgrade.device_id.clone(),
        upgrade.timestamp.clone(),
        upgrade.signature.clone(),
    ) {
        (Some(d), Some(t), Some(s)) => (d, t, s),
        _ => return, // structurally rejected already; defensive.
    };

    // Look up the device record at the api.
    let Some(record) = lookup_device(shared, &device_id).await else {
        eprintln!("device {device_id}: unknown to api; refused");
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

    {
        let mut state = shared.state.lock().await;
        state.add_device(&device_id, &owner, out_tx.clone());
    }
    let _ = out_tx.send(
        json!({"type":"authenticated","device_id":device_id,"heartbeat_interval_s":30}).to_string(),
    );
    report_status(shared, &device_id, true).await;
    eprintln!("device {device_id}: authenticated (owner {owner})");

    while let Some(Ok(message)) = source.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
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
                let clients = {
                    let mut state = shared.state.lock().await;
                    state.set_device_transport_key(&device_id, &key)
                };
                let notice =
                    json!({"type":"device_key","device_id":device_id,"transport_public_key":key})
                        .to_string();
                for client in clients {
                    let _ = client.send(notice.clone());
                }
            }
            "heartbeat" => {}
            "session_accept" | "e2ee_envelope" => {
                if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                    let target = {
                        let state = shared.state.lock().await;
                        state.client_out_for_device_frame(session_id, &device_id)
                    };
                    if let Some(client) = target {
                        let _ = client.send(text);
                    }
                }
            }
            _ => {}
        }
    }

    {
        let mut state = shared.state.lock().await;
        state.remove_device(&device_id);
    }
    report_status(shared, &device_id, false).await;
}

async fn serve_client(
    shared: &Arc<Shared>,
    out_tx: Outbound,
    source: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) {
    // The browser's first frame must authenticate with a gateway token.
    let user_id = match next_text(source).await.and_then(|t| {
        serde_json::from_str::<Value>(&t).ok().and_then(|m| {
            if m.get("type").and_then(Value::as_str) == Some("authenticate") {
                m.get("token").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
    }) {
        Some(token) => match lookup_gateway_token(shared, &token).await {
            Some(user_id) => user_id,
            None => {
                eprintln!("client: invalid gateway token; refused");
                return;
            }
        },
        None => {
            eprintln!("client: missing authenticate frame; refused");
            return;
        }
    };

    let client_id = {
        let mut state = shared.state.lock().await;
        state.add_client(&user_id, out_tx.clone())
    };
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

    while let Some(Ok(message)) = source.next().await {
        let Message::Text(text) = message else {
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
                // The target device id is inside the session_init payload.
                let device_id = msg
                    .get("session_init")
                    .and_then(|v| v.get("device_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let target = {
                    let mut state = shared.state.lock().await;
                    state.open_session(session_id, client_id, device_id)
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

    let mut state = shared.state.lock().await;
    state.remove_client(client_id);
}

// --- api lookups --------------------------------------------------------------

async fn lookup_device(shared: &Arc<Shared>, device_id: &str) -> Option<DeviceRecord> {
    let url = format!(
        "{}/internal/devices/{device_id}",
        shared.api_url.trim_end_matches('/')
    );
    let resp = shared.http.get(&url).send().await.ok()?;
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

async fn lookup_gateway_token(shared: &Arc<Shared>, token: &str) -> Option<String> {
    let url = format!(
        "{}/internal/gateway-token/{token}",
        shared.api_url.trim_end_matches('/')
    );
    let resp = shared.http.get(&url).send().await.ok()?;
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
        shared.api_url.trim_end_matches('/')
    );
    let _ = shared
        .http
        .post(&url)
        .json(&json!({ "online": online }))
        .send()
        .await;
}

async fn next_text(
    source: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> Option<String> {
    while let Some(Ok(message)) = source.next().await {
        match message {
            Message::Text(t) => return Some(t),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
    None
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
