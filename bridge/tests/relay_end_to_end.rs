//! End-to-end: browser → relay → bridge → relay → browser, fully E2E encrypted.
//!
//! A mock relay (faithful to the documented `/ws/device` frame contract) sits
//! between the real bridge relay-client and a simulated browser. The browser
//! bootstraps an encrypted session, sends a request frame, and reads the bridge's
//! encrypted response — proving the whole transport path works, relay-blind.

use std::sync::Arc;

use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::transport::{self, Envelope, FrameFields, OuterFields, SessionInit};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Receive the next value or fail the test on a 10s timeout.
async fn recv(rx: &mut mpsc::Receiver<Value>) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
        .await
        .expect("no timeout")
        .expect("channel open")
}

/// A mock relay that proxies between the connected device and the test's browser
/// channels. It never inspects the encrypted envelopes — exactly like the real one.
async fn mock_relay(
    listener: TcpListener,
    mut browser_to_device: mpsc::Receiver<Value>,
    device_to_browser: mpsc::Sender<Value>,
    transport_key: mpsc::Sender<String>,
) {
    let (tcp, _) = listener.accept().await.expect("device connects");
    let ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("ws handshake");
    let (mut sink, mut source) = ws.split();

    // Greet the device so it uploads its transport key and starts heartbeating.
    sink.send(Message::Text(
        json!({"type": "authenticated", "device_id": "dev-1", "heartbeat_interval_s": 30})
            .to_string(),
    ))
    .await
    .unwrap();

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

#[tokio::test]
async fn browser_relay_bridge_round_trip_is_e2e_encrypted() {
    // The device's stable identity.
    let identity = DeviceIdentity {
        device_id: "dev-1".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };

    // Stand up the mock relay.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}/ws/device");

    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(16);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(16);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));

    // Run the real bridge relay-client; its handler echoes the request payload.
    let handler: relay::FrameHandler =
        Arc::new(|_sender, frame| json!({ "echo": frame.payload, "ok": true }));
    let bridge = {
        let identity = identity.clone();
        tokio::spawn(async move { relay::run(&url, &identity, handler).await })
    };

    // 1. The device uploaded its transport key; the browser learns it.
    let device_transport_pub =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();
    assert_eq!(device_transport_pub, identity.transport.public_key_b64);

    // 2. Browser bootstraps a session: wrap a fresh key to the device.
    let session_id = "sess-1";
    let session_key = transport::generate_session_key();
    let wrapped = transport::wrap_session_key(&device_transport_pub, &session_key).unwrap();
    to_device_tx
        .send(json!({
            "type": "session_init",
            "session_id": session_id,
            "session_init": serde_json::to_value(SessionInit {
                session_id: session_id.into(),
                device_id: "dev-1".into(),
                wrapped_session_key: wrapped,
            }).unwrap(),
        }))
        .await
        .unwrap();

    // 3. Device proves receipt with an encrypted session_accept.
    let accept = recv(&mut from_device_rx).await;
    assert_eq!(accept["type"], "session_accept");
    let accept_env: Envelope = serde_json::from_value(accept["envelope"].clone()).unwrap();
    transport::verify_session_accept(&session_key, &accept_env, session_id)
        .expect("browser verifies session_accept");

    // 4. Browser sends an encrypted request frame.
    let request = transport::encrypt_frame(
        &session_key,
        &OuterFields {
            session_id: session_id.into(),
            route_to: "device:dev-1".into(),
        },
        &FrameFields {
            frame_type: "data".into(),
            sender: "client".into(),
            payload: json!({ "method": "ping", "n": 1 }),
            message_id: None,
            created_at: None,
        },
        None,
    )
    .unwrap();
    to_device_tx
        .send(json!({
            "type": "e2ee_envelope",
            "session_id": session_id,
            "envelope": serde_json::to_value(&request).unwrap(),
        }))
        .await
        .unwrap();

    // 5. Bridge decrypts, the handler echoes, and the response comes back encrypted.
    let response = recv(&mut from_device_rx).await;
    assert_eq!(response["type"], "e2ee_envelope");
    let response_env: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    let frame = transport::decrypt_envelope(&session_key, &response_env).unwrap();
    assert_eq!(frame.sender, "device");
    assert_eq!(
        frame.payload,
        json!({ "echo": { "method": "ping", "n": 1 }, "ok": true })
    );

    // Tear down: dropping the browser sender lets the bridge/relay wind down.
    drop(to_device_tx);
    bridge.abort();
}
