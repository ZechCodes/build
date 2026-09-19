//! What `online` in the account's device list is allowed to mean.
//!
//! It is read as "I can dial this machine", and until this landed it answered
//! "the daemon process is running": the heartbeat went to the api over its own
//! HTTPS connection and knew nothing about the relay socket a browser actually
//! finds the device on. On 2026-09-19 one of Zech's machines had its relay
//! socket severed at 18:54:03Z and did not get another until 20:38:15Z, yet its
//! `last_seen_at` advanced to 19:37:18Z — forty-three minutes of a device the
//! api called online that no browser could open a session to, and twelve dials
//! the relay rejected inside that window to prove it.
//!
//! So the two are tied together here: the relay socket raises the flag when the
//! relay authenticates it and drops it when the socket ends, and the beat goes
//! out only while it is raised (`bridge/src/reachability.rs`). These tests are
//! about the tie, not about either half.

use std::sync::Arc;
use std::time::Duration;

use build_bridge::carrier::{testing, FrameHandler, FrameIntake};
use build_bridge::reachability::Reachability;
use build_bridge::relay::{self, RelayError};
use build_bridge::transport;
use common::{bind_relay, device_identity, greet_device, request_message, session_init_message};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

mod common;

/// Short enough that a give-up is a test rather than a wait: the hand-over
/// deadline is one of these, the silence deadline three.
const HEARTBEAT_INTERVAL_S: u64 = 1;

/// A device that answers instantly, for the tests that are about the socket.
fn idle_intake() -> Arc<FrameIntake> {
    FrameIntake::new(
        FrameHandler::new(
            build_bridge::timing::FrameClock::new(),
            |_sender, _frame, _timer| json!({ "ok": true }),
        ),
        transport::generate_transport_keypair(),
    )
}

/// An intake whose one worker never finishes a frame and whose queue holds one
/// more — a stand-in for the handler pool being full, which is the state the
/// daemon's own pool reaches under load.
fn wedged_intake() -> Arc<FrameIntake> {
    FrameIntake::with_pool(
        FrameHandler::new(
            build_bridge::timing::FrameClock::new(),
            |_sender, _frame, _timer| {
                // The worker runs handlers on a blocking thread, so blocking one
                // is what taking a worker out of the pool looks like. Long past
                // the hand-over deadline, never forever.
                std::thread::sleep(Duration::from_secs(5));
                json!({ "ok": true })
            },
        ),
        transport::generate_transport_keypair(),
        1,
        1,
    )
}

/// Wait for the device to read `want`, or give up. Polled rather than awaited
/// because the flag is written on the relay client's own task.
async fn reads_as(reachable: &Reachability, want: bool) -> bool {
    let deadline = tokio::time::Instant::now() + testing::PATIENCE;
    while tokio::time::Instant::now() < deadline {
        if reachable.is_reachable() == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// A relay that greets the device and then holds the socket open, answering
/// pings, until the test drops it.
async fn relay_holding_the_socket(listener: TcpListener) {
    let mut ws = greet_device(listener, HEARTBEAT_INTERVAL_S).await;
    while let Some(Ok(_)) = ws.next().await {}
}

/// A relay that completes the WebSocket handshake and then says nothing: the
/// socket is up, and this device has not been authorised onto it.
async fn relay_that_never_greets(listener: TcpListener) {
    let (tcp, _) = listener.accept().await.expect("device connects");
    let mut ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("ws handshake");
    while let Some(Ok(_)) = ws.next().await {}
}

#[tokio::test]
async fn a_device_becomes_reachable_when_the_relay_authenticates_its_socket() {
    let (listener, url) = bind_relay().await;
    let socket = tokio::spawn(relay_holding_the_socket(listener));
    let reachable = Reachability::unreachable();

    let running = {
        let reachable = reachable.clone();
        tokio::spawn(async move {
            relay::run(&url, &device_identity(), idle_intake(), &reachable).await
        })
    };

    assert!(
        reads_as(&reachable, true).await,
        "the greeting is the moment a browser could reach this device"
    );
    running.abort();
    socket.abort();
}

#[tokio::test]
async fn a_socket_the_relay_never_authenticated_leaves_the_device_unreachable() {
    let (listener, url) = bind_relay().await;
    let socket = tokio::spawn(relay_that_never_greets(listener));
    let reachable = Reachability::unreachable();

    let running = {
        let reachable = reachable.clone();
        tokio::spawn(async move {
            relay::run(&url, &device_identity(), idle_intake(), &reachable).await
        })
    };

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !reachable.is_reachable(),
        "a TCP connection is not a rendezvous: nothing can route to this device yet"
    );
    running.abort();
    socket.abort();
}

/// The bug, in one test: the socket goes and the device stops saying it is
/// there. Whatever ended it — severed, closed, dropped mid-session — the flag
/// falls, and the next heartbeat window is the last one the api calls this
/// device online.
#[tokio::test]
async fn a_device_whose_relay_socket_drops_stops_being_reachable() {
    let (listener, url) = bind_relay().await;
    let socket = tokio::spawn(relay_holding_the_socket(listener));
    let reachable = Reachability::unreachable();

    let running = {
        let reachable = reachable.clone();
        tokio::spawn(async move {
            relay::run(&url, &device_identity(), idle_intake(), &reachable).await
        })
    };
    assert!(reads_as(&reachable, true).await, "the device connected");

    socket.abort(); // the relay is gone, as it was for b02b5ba1 at 18:54:03Z

    assert!(
        reads_as(&reachable, false).await,
        "a device with no way in must not go on reporting itself online"
    );
    running.abort();
}

/// The read loop hands frames over to a bounded pool, so a pool that is full
/// blocks it — and a blocked read loop sends no pongs, which is what had the
/// relay log `pings unanswered (read loop dead) for 90s; severing`. The socket
/// is given up at one heartbeat interval instead, well inside the relay's own
/// window, so the daemon redials rather than sitting behind a socket the relay
/// has already written off.
#[tokio::test]
async fn a_bridge_that_cannot_drain_its_socket_gives_it_up_and_reads_unreachable() {
    let (listener, url) = bind_relay().await;
    let intake = wedged_intake();
    let transport_public_key = intake.transport_public_key().to_string();
    let session_key = transport::generate_session_key();
    let reachable = Reachability::unreachable();

    let socket = tokio::spawn(async move {
        let mut ws = greet_device(listener, HEARTBEAT_INTERVAL_S).await;
        ws.send(Message::Text(
            session_init_message("sess-wedge", &transport_public_key, &session_key).to_string(),
        ))
        .await
        .unwrap();
        // One frame for the worker, one for the queue, one with nowhere to go:
        // the third is the one the read loop has to wait on.
        for n in 0..3 {
            ws.send(Message::Text(
                request_message(
                    &session_key,
                    "sess-wedge",
                    json!({ "method": "rtc.ice", "n": n }),
                )
                .to_string(),
            ))
            .await
            .unwrap();
        }
        while let Some(Ok(_)) = ws.next().await {}
    });

    let outcome = tokio::time::timeout(
        testing::PATIENCE,
        relay::run(&url, &device_identity(), intake, &reachable),
    )
    .await
    .expect("a socket it cannot drain is one the bridge gives up, not one it holds");

    match outcome {
        Err(RelayError::Wedged(after)) => assert_eq!(
            after,
            Duration::from_secs(HEARTBEAT_INTERVAL_S),
            "given up after one heartbeat interval"
        ),
        other => panic!("expected RelayError::Wedged, got {other:?}"),
    }
    assert!(
        !reachable.is_reachable(),
        "a bridge that cannot answer on its socket must not report itself online"
    );
    socket.abort();
}
