//! A browser as the relay carries it: the mock relay a test stands the real
//! bridge up against, and the two message types a client puts on that wire.
//!
//! One home for both, because every carrier test starts the same way — a
//! device authenticated against a relay, a session minted over it — and only
//! then diverges into what it is actually about. Each test binary compiles its
//! own copy and uses the part it needs, so what another binary uses is not
//! dead here.
#![allow(dead_code)]

use std::sync::Arc;

use build_bridge::carrier::testing;
use build_bridge::carrier::{FrameHandler, FrameIntake};
use build_bridge::relay::{self, DeviceIdentity, RelayError};
use build_bridge::transport::{self, DATA_FRAME_TYPE};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// A browser opening a session over the relay.
pub fn session_init_message(
    session_id: &str,
    transport_public_key: &str,
    session_key: &str,
) -> Value {
    json!({
        "type": "session_init",
        "session_id": session_id,
        "session_init": serde_json::to_value(
            testing::session_init(session_id, transport_public_key, session_key),
        ).expect("a session_init serializes"),
    })
}

/// One encrypted request from that browser, in the relay's envelope wrapper.
pub fn request_message(session_key: &str, session_id: &str, payload: Value) -> Value {
    json!({
        "type": "e2ee_envelope",
        "session_id": session_id,
        "envelope": serde_json::to_value(
            testing::client_request(session_key, session_id, DATA_FRAME_TYPE, payload),
        ).expect("an envelope serializes"),
    })
}

/// Receive the next value or fail the test on a 10s timeout.
pub async fn recv(rx: &mut mpsc::Receiver<Value>) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("no timeout")
        .expect("channel open")
}

/// The heartbeat a relay asks a device for unless the test is about the
/// heartbeat itself.
pub const HEARTBEAT_INTERVAL_S: u64 = 30;

/// A socket for a mock relay, and the URL the device reaches it on.
pub async fn bind_relay() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    (listener, url)
}

/// Take the device's connection and greet it as the relay does, so it uploads
/// its transport key and starts heartbeating at `heartbeat_interval_s`.
pub async fn greet_device(
    listener: TcpListener,
    heartbeat_interval_s: u64,
) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
    let (tcp, _) = listener.accept().await.expect("device connects");
    let mut ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("ws handshake");
    ws.send(Message::Text(
        json!({
            "type": "authenticated",
            "device_id": testing::DEVICE_ID,
            "heartbeat_interval_s": heartbeat_interval_s,
        })
        .to_string(),
    ))
    .await
    .unwrap();
    ws
}

/// A mock relay that proxies between the connected device and the test's browser
/// channels. It never inspects the encrypted envelopes — exactly like the real one.
async fn mock_relay(
    listener: TcpListener,
    mut browser_to_device: mpsc::Receiver<Value>,
    device_to_browser: mpsc::Sender<Value>,
    transport_key: mpsc::Sender<String>,
) {
    let (mut sink, mut source) = greet_device(listener, HEARTBEAT_INTERVAL_S).await.split();

    let mut inject_open = true;
    loop {
        tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let v: Value = serde_json::from_str(&text).unwrap();
                    match v.get("type").and_then(Value::as_str) {
                        Some("transport_key") => {
                            let key = v["transport_public_key"].as_str().unwrap().to_string();
                            let _ = transport_key.send(key).await;
                        }
                        Some("heartbeat") => {}
                        Some("session_accept") | Some("e2ee_envelope") => {
                            let _ = device_to_browser.send(v).await;
                        }
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                _ => break, // device closed
            },
            injected = browser_to_device.recv(), if inject_open => match injected {
                Some(v) => sink.send(Message::Text(v.to_string())).await.unwrap(),
                None => inject_open = false,
            },
        }
    }
}

/// The device's stable identity, freshly minted.
pub fn device_identity() -> DeviceIdentity {
    DeviceIdentity {
        device_id: testing::DEVICE_ID.into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
    }
}

/// An intake holding a fresh device transport keypair — the key browsers wrap
/// their session keys to, and the one the relay socket uploads.
pub fn test_intake(handler: FrameHandler) -> Arc<FrameIntake> {
    FrameIntake::new(handler, transport::generate_transport_keypair())
}

/// A bridge stood up against one mock relay socket, as the test's browser sees
/// it: the two halves of the relay wire, the transport public key the device
/// uploaded, and the two tasks behind them.
pub struct ConnectedDevice {
    pub to_device: mpsc::Sender<Value>,
    pub from_device: mpsc::Receiver<Value>,
    pub transport_public_key: String,
    pub bridge: JoinHandle<Result<(), RelayError>>,
    pub relay_socket: JoinHandle<()>,
}

/// Bind a mock relay, run the real bridge relay-client against it with
/// `intake`, and wait until the device has authenticated and uploaded its
/// transport key.
pub async fn connected_device(
    intake: Arc<FrameIntake>,
    identity: &DeviceIdentity,
) -> ConnectedDevice {
    let (listener, url) = bind_relay().await;
    let (to_device, to_device_rx) = mpsc::channel::<Value>(64);
    let (from_device_tx, from_device) = mpsc::channel::<Value>(64);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    let relay_socket = tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));
    let bridge = {
        let identity = identity.clone();
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
    };
    let transport_public_key =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();
    ConnectedDevice {
        to_device,
        from_device,
        transport_public_key,
        bridge,
        relay_socket,
    }
}
