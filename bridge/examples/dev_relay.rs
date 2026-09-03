//! A self-contained dev relay for the browser end-to-end demo.
//!
//! Runs a minimal two-sided relay (faithful to the frame contract: it forwards
//! opaque envelopes and never decrypts) plus an in-process echo "device" using
//! the real bridge relay client. A real browser (or the Node harness in `web/`)
//! connects to `/ws/client`, bootstraps an E2EE session with the device, and
//! exchanges frames — proving browser → relay → bridge → relay → browser.
//!
//! Run: `cargo run --example dev_relay` (port from DEV_RELAY_PORT, default 8799).

use std::sync::Arc;

use build_bridge::carrier::{FrameHandler, FrameIntake};
use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::transport;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

type Outbound = mpsc::UnboundedSender<Message>;

/// Shared relay routing state: the two peers' write channels and the device's
/// transport key (served to clients so they can wrap a session key to it).
#[derive(Default)]
struct RelayState {
    device_out: Option<Outbound>,
    client_out: Option<Outbound>,
    device_transport_key: Option<String>,
}

type Shared = Arc<Mutex<RelayState>>;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("DEV_RELAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8799);
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let url = format!("ws://127.0.0.1:{port}");
    let state: Shared = Arc::new(Mutex::new(RelayState::default()));

    // Spawn the in-process device: the real bridge relay client with an echo
    // handler standing in for the orchestrator.
    let identity = DeviceIdentity {
        device_id: "dev-relay-device".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };
    {
        let device_url = format!("{url}/ws/device");
        tokio::spawn(async move {
            let handler: FrameHandler =
                Arc::new(|_sender, frame| json!({ "echo": frame.payload, "from": "bridge" }));
            let intake = FrameIntake::new(handler);
            if let Err(e) = relay::run(&device_url, &identity, intake).await {
                eprintln!("device exited: {e}");
            }
        });
    }

    println!("DEV_RELAY_LISTENING {url}");
    println!("  device  → {url}/ws/device");
    println!("  client  → {url}/ws/client");

    loop {
        let (tcp, _) = listener.accept().await.unwrap();
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = serve(tcp, state).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

/// Accept one WebSocket, routing by path: `/ws/device` or `/ws/client`.
// The tungstenite accept callback's error type (ErrorResponse) is large and
// fixed by the API; the lint isn't actionable here.
#[allow(clippy::result_large_err)]
async fn serve(tcp: TcpStream, state: Shared) -> Result<(), Box<dyn std::error::Error>> {
    let path = Arc::new(std::sync::Mutex::new(String::new()));
    let path_capture = Arc::clone(&path);
    let ws = tokio_tungstenite::accept_hdr_async(tcp, move |req: &Request, resp: Response| {
        *path_capture.lock().unwrap() = req.uri().path().to_string();
        Ok(resp)
    })
    .await?;
    let path = path.lock().unwrap().clone();

    let (mut sink, mut source) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();

    // Pump this peer's outbound queue to its socket.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    let is_device = path == "/ws/device";
    {
        let mut s = state.lock().await;
        if is_device {
            s.device_out = Some(out_tx.clone());
            // Greet the device so it uploads its transport key and heartbeats.
            let _ = out_tx.send(Message::Text(
                json!({"type":"authenticated","device_id":"dev-relay-device","heartbeat_interval_s":30}).to_string(),
            ));
        } else {
            s.client_out = Some(out_tx.clone());
            // Tell the client which device key to wrap to (once known).
            if let Some(key) = &s.device_transport_key {
                let _ = out_tx.send(Message::Text(
                    json!({"type":"device_key","transport_public_key":key}).to_string(),
                ));
            }
        }
    }

    while let Some(message) = source.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

        if is_device {
            match msg_type {
                "transport_key" => {
                    let key = msg
                        .get("transport_public_key")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let mut s = state.lock().await;
                    s.device_transport_key = Some(key.clone());
                    // If a client is already waiting, hand it the key now.
                    if let Some(client) = &s.client_out {
                        let _ = client.send(Message::Text(
                            json!({"type":"device_key","transport_public_key":key}).to_string(),
                        ));
                    }
                }
                "heartbeat" => {}
                "session_accept" | "e2ee_envelope" => {
                    forward(&state, /* to_device */ false, text).await;
                }
                _ => {}
            }
        } else {
            match msg_type {
                "session_init" | "e2ee_envelope" => {
                    forward(&state, /* to_device */ true, text).await;
                }
                _ => {}
            }
        }
    }

    writer.abort();
    Ok(())
}

/// Forward a verbatim frame to the other peer (the relay never decrypts it).
async fn forward(state: &Shared, to_device: bool, text: String) {
    let s = state.lock().await;
    let target = if to_device {
        &s.device_out
    } else {
        &s.client_out
    };
    if let Some(tx) = target {
        let _ = tx.send(Message::Text(text));
    }
}
