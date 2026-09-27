//! What `online` in the account's device list is allowed to mean.
//!
//! It is read as "I can dial this machine", and until this landed it answered
//! "the daemon process is running": the heartbeat went to the api over its own
//! HTTPS connection and knew nothing about the relay socket a browser actually
//! finds the device on. On 2026-09-19 a user's machine had its relay
//! socket severed at 18:54:03Z and did not get another until 20:38:15Z, yet its
//! `last_seen_at` advanced to 19:37:18Z — forty-three minutes of a device the
//! api called online that no browser could open a session to, and twelve dials
//! the relay rejected inside that window to prove it.
//!
//! So the two are tied together here: the relay socket raises the flag when the
//! relay authenticates it and drops it when the socket ends, and the beat goes
//! out only while it is raised (`bridge/src/reachability.rs`). These tests are
//! about the tie, not about either half.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use build_bridge::carrier::{testing, FrameHandler, FrameIntake};
use build_bridge::reachability::Reachability;
use build_bridge::relay;
use build_bridge::timing::FrameClock;
use build_bridge::transport;
use common::{bind_relay, device_identity, greet_device};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
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
/// daemon's own pool reaches under load. `entered` counts the frames a handler
/// started on, and the clock reports the queue, so a test can say the pool IS
/// full rather than assume it. The worker holds its frame until the returned
/// sender is dropped.
fn wedged_intake(
    clock: Arc<FrameClock>,
    entered: Arc<AtomicUsize>,
) -> (Arc<FrameIntake>, std::sync::mpsc::Sender<()>) {
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let blocked = Mutex::new(blocked);
    let intake = FrameIntake::with_pool(
        FrameHandler::new(clock, move |_sender, _frame, _timer| {
            entered.fetch_add(1, Ordering::SeqCst);
            // The worker runs handlers on a blocking thread, so blocking one
            // is what taking a worker out of the pool looks like. Dropping
            // `release` unblocks it, including when the test panics.
            let _ = blocked.lock().unwrap().recv();
            json!({ "ok": true })
        }),
        transport::generate_transport_keypair(),
        1,
        1,
    );
    (intake, release)
}

/// Wait for `condition`, or give up.
async fn settles(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + testing::PATIENCE;
    while tokio::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
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

/// A relay that completes the WebSocket handshake but never authenticates the
/// device. A pong proves the device has processed traffic on this socket before
/// the test checks reachability.
async fn relay_that_never_greets(listener: TcpListener, ready: oneshot::Sender<()>) {
    let (tcp, _) = listener.accept().await.expect("device connects");
    let mut ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("ws handshake");
    ws.send(Message::Ping(Vec::new()))
        .await
        .expect("ping sends");
    while let Some(Ok(message)) = ws.next().await {
        if matches!(message, Message::Pong(_)) {
            let _ = ready.send(());
            break;
        }
    }
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
    let (ready_tx, ready_rx) = oneshot::channel();
    let socket = tokio::spawn(relay_that_never_greets(listener, ready_tx));
    let reachable = Reachability::unreachable();

    let running = {
        let reachable = reachable.clone();
        tokio::spawn(async move {
            relay::run(&url, &device_identity(), idle_intake(), &reachable).await
        })
    };

    tokio::time::timeout(testing::PATIENCE, ready_rx)
        .await
        .expect("the device processes the relay's ping")
        .expect("the relay observes its pong");
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

/// The read loop hands frames to a queue of the wire's own and never waits on
/// the handler pool, so a pool that is full — the state the daemon's own pool
/// reaches under load — costs the socket nothing: the heartbeat goes on, the
/// device stays reachable, and the socket is held rather than given up. (A
/// read loop that waited on the pool sent no heartbeat, the relay logged
/// `pings unanswered (read loop dead) for 90s; severing`, and the socket was
/// given up after one heartbeat interval to be redialled — a redial the phone
/// had to wait out.)
///
/// The pool is filled the way a browser fills it: over a DataChannel. A
/// request over the relay is refused before the dispatcher (the relay is not
/// a data plane), so `term.list` over the socket would leave the pool idle
/// and the test checking nothing — the round-2 review of task #128 caught
/// exactly that. The frames the pool cannot take wait on the wire's queue,
/// and past its depth are refused as busy (`carrier`'s tests).
#[tokio::test]
async fn a_bridge_whose_pool_is_full_keeps_its_socket_and_stays_reachable() {
    let (listener, url) = bind_relay().await;
    let clock = FrameClock::new();
    let entered = Arc::new(AtomicUsize::new(0));
    let (intake, release_worker) = wedged_intake(Arc::clone(&clock), Arc::clone(&entered));
    let reachable = Reachability::unreachable();
    let heartbeats = Arc::new(AtomicUsize::new(0));

    let socket = {
        let heartbeats = Arc::clone(&heartbeats);
        tokio::spawn(async move {
            let mut ws = greet_device(listener, HEARTBEAT_INTERVAL_S).await;
            while let Some(Ok(message)) = ws.next().await {
                if let Message::Text(text) = message {
                    let heard: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    if heard["type"] == "heartbeat" {
                        heartbeats.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    let running = {
        let (intake, reachable) = (Arc::clone(&intake), reachable.clone());
        tokio::spawn(async move { relay::run(&url, &device_identity(), intake, &reachable).await })
    };
    assert!(reads_as(&reachable, true).await, "the device connected");

    // A browser's session over a DataChannel: one frame for the worker, one
    // for the queue, one with nowhere to go but the wire's own queue.
    let wire = testing::ChannelWire::open();
    let session_key = transport::generate_session_key();
    wire.open_session(
        &intake,
        "sess-wedge",
        &testing::session_init("sess-wedge", intake.transport_public_key(), &session_key),
    )
    .expect("the session opened over the channel");
    for id in 0..3 {
        wire.accept(
            &intake,
            testing::client_request(
                &session_key,
                "sess-wedge",
                "data",
                json!({ "id": id, "method": "term.list" }),
            ),
        )
        .await
        .expect("the frame was admitted");
    }
    let pool_is_full = || entered.load(Ordering::SeqCst) == 1 && clock.stats()["queue_depth"] == 2;
    assert!(
        settles(pool_is_full).await,
        "the worker is on a frame, the queue holds the second, and the wire's \
         admitter holds the third at the queue's door: entered={} depth={}",
        entered.load(Ordering::SeqCst),
        clock.stats()["queue_depth"]
    );

    // Well past the one interval a socket the reader could not drain used to
    // be given up at — and the handler holds its worker until it is released.
    let heard_before = heartbeats.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(3 * HEARTBEAT_INTERVAL_S)).await;
    assert!(
        pool_is_full(),
        "the pool stayed full for the whole wait: entered={} depth={}",
        entered.load(Ordering::SeqCst),
        clock.stats()["queue_depth"]
    );
    assert!(
        !running.is_finished(),
        "the socket is held while the pool is full, not given up"
    );
    assert!(
        heartbeats.load(Ordering::SeqCst) >= heard_before + 2,
        "the heartbeat went on over a full pool: {} before, {} after",
        heard_before,
        heartbeats.load(Ordering::SeqCst)
    );
    assert!(
        reachable.is_reachable(),
        "a bridge whose pool is full still answers on its socket, and says so"
    );
    wire.close(&intake);
    running.abort();
    drop(release_worker);
    socket.abort();
}
