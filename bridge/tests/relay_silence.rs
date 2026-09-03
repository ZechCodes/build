use std::sync::Arc;
use std::time::Duration;

use build_bridge::carrier::{FrameHandler, FrameIntake};
use build_bridge::relay::{self, DeviceIdentity, RelayError};
use build_bridge::transport;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const HEARTBEAT_INTERVAL_S: u64 = 1;

fn test_identity() -> DeviceIdentity {
    DeviceIdentity {
        device_id: "dev-silence".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
    }
}

fn idle_intake() -> Arc<FrameIntake> {
    FrameIntake::new(idle_handler(), transport::generate_transport_keypair())
}

fn idle_handler() -> FrameHandler {
    Arc::new(|_sender, frame| json!({ "echo": frame.payload }))
}

async fn bind_relay() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    (listener, url)
}

async fn greet_device(
    listener: TcpListener,
) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
    let (tcp, _) = listener.accept().await.expect("device connects");
    let mut ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("ws handshake");
    ws.send(Message::Text(
        json!({
            "type": "authenticated",
            "device_id": "dev-silence",
            "heartbeat_interval_s": HEARTBEAT_INTERVAL_S,
        })
        .to_string(),
    ))
    .await
    .unwrap();
    ws
}

async fn relay_that_stops_reading(listener: TcpListener) {
    let ws = greet_device(listener).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    drop(ws);
}

async fn relay_that_only_answers_pings(listener: TcpListener) {
    let mut ws = greet_device(listener).await;
    while let Some(Ok(_)) = ws.next().await {}
}

#[tokio::test]
async fn a_relay_that_goes_silent_is_treated_as_disconnected() {
    let (listener, url) = bind_relay().await;
    tokio::spawn(relay_that_stops_reading(listener));

    let outcome = tokio::time::timeout(
        Duration::from_secs(HEARTBEAT_INTERVAL_S * 3 + 5),
        relay::run(&url, &test_identity(), idle_intake()),
    )
    .await
    .expect("the client gives up on a silent relay instead of waiting forever");

    match outcome {
        Err(RelayError::Silent(after)) => {
            assert_eq!(after, Duration::from_secs(HEARTBEAT_INTERVAL_S * 3))
        }
        other => panic!("expected RelayError::Silent, got {other:?}"),
    }
}

#[tokio::test]
async fn a_quiet_relay_that_still_answers_pings_keeps_the_session() {
    let (listener, url) = bind_relay().await;
    tokio::spawn(relay_that_only_answers_pings(listener));

    let still_running = tokio::time::timeout(
        Duration::from_secs(HEARTBEAT_INTERVAL_S * 3 + 3),
        relay::run(&url, &test_identity(), idle_intake()),
    )
    .await
    .is_err();

    assert!(
        still_running,
        "pong replies are proof of life; the client must not give up"
    );
}
