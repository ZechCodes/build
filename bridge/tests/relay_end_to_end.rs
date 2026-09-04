//! End-to-end: browser → relay → bridge → relay → browser, fully E2E encrypted.
//!
//! A mock relay (faithful to the documented `/ws/device` frame contract) sits
//! between the real bridge relay-client and a simulated browser. The browser
//! bootstraps an encrypted session, sends a request frame, and reads the bridge's
//! encrypted response — proving the whole transport path works, relay-blind.

use std::sync::Arc;

use build_bridge::carrier::testing::{next_report, reporting_handler};
use build_bridge::carrier::FrameHandler;
use build_bridge::transport::{self, Envelope};
use common::{
    connected_device, device_identity, recv, request_message, session_init_message, test_intake,
    ConnectedDevice,
};
use serde_json::json;

mod common;

#[tokio::test]
async fn browser_relay_bridge_round_trip_is_e2e_encrypted() {
    let identity = device_identity();
    // Run the real bridge relay-client; its handler echoes the request payload.
    let handler: FrameHandler =
        Arc::new(|_sender, frame| json!({ "echo": frame.payload, "ok": true }));
    let intake = test_intake(handler);
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key,
        bridge,
        relay_socket: _,
    } = connected_device(intake.clone(), &identity).await;

    // 1. The device uploaded the intake's transport key; the browser learns it.
    assert_eq!(transport_public_key, intake.transport_public_key());

    // 2. Browser bootstraps a session: wrap a fresh key to the device.
    let session_id = "sess-1";
    let session_key = transport::generate_session_key();
    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &session_key,
        ))
        .await
        .unwrap();

    // 3. Device proves receipt with an encrypted session_accept.
    let accept = recv(&mut from_device).await;
    assert_eq!(accept["type"], "session_accept");
    let accept_env: Envelope = serde_json::from_value(accept["envelope"].clone()).unwrap();
    transport::verify_session_accept(&session_key, &accept_env, session_id)
        .expect("browser verifies session_accept");

    // 4. Browser sends an encrypted request frame.
    to_device
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping", "n": 1 }),
        ))
        .await
        .unwrap();

    // 5. Bridge decrypts, the handler echoes, and the response comes back encrypted.
    let response = recv(&mut from_device).await;
    assert_eq!(response["type"], "e2ee_envelope");
    let response_env: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    let frame = transport::decrypt_envelope(&session_key, &response_env).unwrap();
    assert_eq!(frame.sender, "device");
    assert_eq!(
        frame.payload,
        json!({ "echo": { "method": "ping", "n": 1 }, "ok": true })
    );

    // Tear down: dropping the browser sender lets the bridge/relay wind down.
    drop(to_device);
    bridge.abort();
}

/// The wedge test: a handler that takes seconds must not stop the device from
/// reading its socket. The browser sends one slow request and then five cheap
/// ones; every cheap answer comes back while the slow one is still working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_handler_does_not_stall_the_socket() {
    let identity = device_identity();
    // `slow` is the board.list-with-a-libgit2-diff of the incident.
    let handler: FrameHandler = Arc::new(|_sender, frame| {
        if frame.payload["method"] == "slow" {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        json!({ "id": frame.payload["id"] })
    });
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key,
        bridge,
        relay_socket: _,
    } = connected_device(test_intake(handler), &identity).await;

    let session_id = "sess-slow";
    let session_key = transport::generate_session_key();
    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &session_key,
        ))
        .await
        .unwrap();
    let accept = recv(&mut from_device).await;
    assert_eq!(accept["type"], "session_accept");

    let ask = |id: u64, method: &str| {
        request_message(
            &session_key,
            session_id,
            json!({ "id": id, "method": method }),
        )
    };

    to_device.send(ask(0, "slow")).await.unwrap();
    for id in 1..=5u64 {
        to_device.send(ask(id, "cheap")).await.unwrap();
    }

    // The five cheap answers come back first — the read loop kept draining.
    let mut answered: Vec<u64> = Vec::new();
    for _ in 0..5 {
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(1), recv(&mut from_device))
                .await
                .expect("cheap answers do not wait for the slow one");
        let envelope: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
        let frame = transport::decrypt_envelope(&session_key, &envelope).unwrap();
        answered.push(frame.payload["id"].as_u64().unwrap());
    }
    answered.sort_unstable();
    assert_eq!(
        answered,
        vec![1, 2, 3, 4, 5],
        "every cheap frame was answered"
    );

    // And the slow one still gets its answer.
    let response = recv(&mut from_device).await;
    let envelope: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    let frame = transport::decrypt_envelope(&session_key, &envelope).unwrap();
    assert_eq!(frame.payload["id"], 0);

    drop(to_device);
    bridge.abort();
}

/// The relay socket is one carrier. When it dies, the sessions that rode nothing
/// else end with it: the app hears each session's synthetic `close`, and the same
/// session id on the next socket is a session the device no longer knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_relay_socket_ends_the_sessions_that_rode_only_it() {
    let identity = device_identity();
    let (handler, mut frames) = reporting_handler();
    let intake = test_intake(handler);
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key,
        bridge,
        relay_socket,
    } = connected_device(intake.clone(), &identity).await;
    let session_id = "sess-carried";
    let session_key = transport::generate_session_key();
    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device).await["type"], "session_accept");
    to_device
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    assert_eq!(next_report(&mut frames).await, format!("data:{session_id}"));

    // The socket goes, with no session_closed and no client close frame.
    relay_socket.abort();
    let _ = bridge.await;

    assert_eq!(
        next_report(&mut frames).await,
        format!("close:{session_id}"),
        "the session that rode only that socket ended with it"
    );

    // The next socket, same intake: the old session id is nobody's.
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key: _,
        bridge,
        relay_socket: _,
    } = connected_device(intake, &identity).await;

    to_device
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), from_device.recv())
            .await
            .is_err(),
        "a frame for a session the device forgot is answered by nothing"
    );
    assert!(frames.try_recv().is_err(), "and reaches no handler");

    drop(to_device);
    bridge.abort();
}

/// A `session_init` for a live session under a fresh key is the frame the spec
/// drops: no `session_accept` comes back and the session keeps the key it was
/// minted with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_init_under_a_different_key_is_refused() {
    let identity = device_identity();
    let (handler, _frames) = reporting_handler();
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key,
        bridge,
        relay_socket: _,
    } = connected_device(test_intake(handler), &identity).await;
    let session_id = "sess-minted";
    let session_key = transport::generate_session_key();
    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device).await["type"], "session_accept");

    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &transport::generate_session_key(),
        ))
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), from_device.recv())
            .await
            .is_err(),
        "the device accepts no session under a key that is not the one it holds"
    );

    to_device
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    let response = recv(&mut from_device).await;
    assert_eq!(response["type"], "e2ee_envelope");
    let envelope: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    transport::decrypt_envelope(&session_key, &envelope)
        .expect("the session still holds the key it was minted with");

    drop(to_device);
    bridge.abort();
}

/// The carrier is released however `run` ends. A caller that drops the future
/// mid-session — a timeout around it, an abort — never reaches the read loop's
/// normal exit, and the sessions riding that socket must end all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_run_ends_the_sessions_that_rode_its_socket() {
    let identity = device_identity();
    let (handler, mut frames) = reporting_handler();
    let ConnectedDevice {
        to_device,
        mut from_device,
        transport_public_key,
        bridge,
        relay_socket: _,
    } = connected_device(test_intake(handler), &identity).await;
    let session_id = "sess-cut";
    let session_key = transport::generate_session_key();
    to_device
        .send(session_init_message(
            session_id,
            &transport_public_key,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device).await["type"], "session_accept");
    to_device
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    assert_eq!(next_report(&mut frames).await, format!("data:{session_id}"));

    // The run future is dropped mid-session, the socket still up.
    bridge.abort();
    let _ = bridge.await;

    assert_eq!(
        next_report(&mut frames).await,
        format!("close:{session_id}"),
        "the session that rode the cancelled socket ended with it"
    );
}
