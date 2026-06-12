//! A standalone dev relay: forwards opaque E2EE envelopes between a device
//! (`/ws/device`) and browser clients (`/ws/client`), and serves the device's
//! transport public key to clients. It never decrypts anything.
//!
//! This is a minimal stand-in for the production `build-relay`; the device-side
//! frame contract is identical, so the bridge connects to either unchanged.
//!
//! Config: `RELAY_PORT` (default 8799). Run: `build-relay` / `cargo run --bin relay`.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

type Outbound = mpsc::UnboundedSender<Message>;

#[derive(Default)]
struct RelayState {
    device_out: Option<Outbound>,
    client_out: Option<Outbound>,
    device_transport_key: Option<String>,
}

type Shared = Arc<Mutex<RelayState>>;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("RELAY_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8799);
    let listener = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
    let state: Shared = Arc::new(Mutex::new(RelayState::default()));

    println!("RELAY_LISTENING ws://0.0.0.0:{port}  (device→/ws/device, client→/ws/client)");

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

/// Accept one WebSocket, routing by path.
// The tungstenite accept callback's error type is large and fixed by the API.
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
            let _ = out_tx.send(Message::Text(
                json!({"type":"authenticated","device_id":"bridge","heartbeat_interval_s":30})
                    .to_string(),
            ));
        } else {
            s.client_out = Some(out_tx.clone());
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
                    if let Some(client) = &s.client_out {
                        let _ = client.send(Message::Text(
                            json!({"type":"device_key","transport_public_key":key}).to_string(),
                        ));
                    }
                }
                "heartbeat" => {}
                "session_accept" | "e2ee_envelope" => forward(&state, false, text).await,
                _ => {}
            }
        } else {
            match msg_type {
                "session_init" | "e2ee_envelope" => forward(&state, true, text).await,
                _ => {}
            }
        }
    }

    writer.abort();
    Ok(())
}

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
