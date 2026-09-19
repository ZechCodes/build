//! A relay that stops reading is a relay the device gives up on, and pong
//! replies are the proof of life that keeps it. The heartbeat interval here is
//! seconds rather than the usual half-minute, so the give-up is a test and not
//! a wait.

use std::sync::Arc;
use std::time::Duration;

use build_bridge::carrier::{FrameHandler, FrameIntake};
use build_bridge::reachability::Reachability;
use build_bridge::relay::{self, RelayError};
use common::{bind_relay, device_identity, greet_device, test_intake};
use futures_util::StreamExt;
use serde_json::json;
use tokio::net::TcpListener;

mod common;

const HEARTBEAT_INTERVAL_S: u64 = 1;

fn idle_intake() -> Arc<FrameIntake> {
    test_intake(idle_handler())
}

fn idle_handler() -> FrameHandler {
    FrameHandler::new(
        build_bridge::timing::FrameClock::new(),
        |_sender, frame, _timer| json!({ "echo": frame.payload }),
    )
}

async fn relay_that_stops_reading(listener: TcpListener) {
    let ws = greet_device(listener, HEARTBEAT_INTERVAL_S).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    drop(ws);
}

async fn relay_that_only_answers_pings(listener: TcpListener) {
    let mut ws = greet_device(listener, HEARTBEAT_INTERVAL_S).await;
    while let Some(Ok(_)) = ws.next().await {}
}

#[tokio::test]
async fn a_relay_that_goes_silent_is_treated_as_disconnected() {
    let (listener, url) = bind_relay().await;
    tokio::spawn(relay_that_stops_reading(listener));

    let outcome = tokio::time::timeout(
        Duration::from_secs(HEARTBEAT_INTERVAL_S * 3 + 5),
        relay::run(
            &url,
            &device_identity(),
            idle_intake(),
            &Reachability::unreachable(),
        ),
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
        relay::run(
            &url,
            &device_identity(),
            idle_intake(),
            &Reachability::unreachable(),
        ),
    )
    .await
    .is_err();

    assert!(
        still_running,
        "pong replies are proof of life; the client must not give up"
    );
}
