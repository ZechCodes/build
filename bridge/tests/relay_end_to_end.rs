//! End-to-end: browser → relay → bridge → relay → browser, fully E2E encrypted.
//!
//! A mock relay (faithful to the documented `/ws/device` frame contract) sits
//! between the real bridge relay-client and a simulated browser. The browser
//! bootstraps an encrypted session, sends a request frame, and reads the bridge's
//! encrypted response — proving the whole transport path works, relay-blind.

use std::sync::Arc;

use build_bridge::carrier::{FrameHandler, FrameIntake};
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
    let handler: FrameHandler =
        Arc::new(|_sender, frame| json!({ "echo": frame.payload, "ok": true }));
    let bridge = {
        let identity = identity.clone();
        let intake = FrameIntake::new(handler);
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
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

/// The wedge test: a handler that takes seconds must not stop the device from
/// reading its socket. The browser sends one slow request and then five cheap
/// ones; every cheap answer comes back while the slow one is still working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_handler_does_not_stall_the_socket() {
    let identity = DeviceIdentity {
        device_id: "dev-1".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}/ws/device");

    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(64);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(64);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));

    // `slow` is the board.list-with-a-libgit2-diff of the incident.
    let handler: FrameHandler = Arc::new(|_sender, frame| {
        if frame.payload["method"] == "slow" {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        json!({ "id": frame.payload["id"] })
    });
    let bridge = {
        let identity = identity.clone();
        let intake = FrameIntake::new(handler);
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
    };

    let device_transport_pub =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();

    let session_id = "sess-slow";
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
    let accept = recv(&mut from_device_rx).await;
    assert_eq!(accept["type"], "session_accept");

    let ask = |id: u64, method: &str| {
        let envelope = transport::encrypt_frame(
            &session_key,
            &OuterFields {
                session_id: session_id.into(),
                route_to: "device:dev-1".into(),
            },
            &FrameFields {
                frame_type: "data".into(),
                sender: "client".into(),
                payload: json!({ "id": id, "method": method }),
                message_id: None,
                created_at: None,
            },
            None,
        )
        .unwrap();
        json!({
            "type": "e2ee_envelope",
            "session_id": session_id,
            "envelope": serde_json::to_value(&envelope).unwrap(),
        })
    };

    to_device_tx.send(ask(0, "slow")).await.unwrap();
    for id in 1..=5u64 {
        to_device_tx.send(ask(id, "cheap")).await.unwrap();
    }

    // The five cheap answers come back first — the read loop kept draining.
    let mut answered: Vec<u64> = Vec::new();
    for _ in 0..5 {
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(1), recv(&mut from_device_rx))
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
    let response = recv(&mut from_device_rx).await;
    let envelope: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    let frame = transport::decrypt_envelope(&session_key, &envelope).unwrap();
    assert_eq!(frame.payload["id"], 0);

    drop(to_device_tx);
    bridge.abort();
}

/// A browser opening a session, in the two messages the relay carries for it.
fn session_init_message(session_id: &str, device_transport_pub: &str, session_key: &str) -> Value {
    let wrapped = transport::wrap_session_key(device_transport_pub, session_key).unwrap();
    json!({
        "type": "session_init",
        "session_id": session_id,
        "session_init": serde_json::to_value(SessionInit {
            session_id: session_id.into(),
            device_id: "dev-1".into(),
            wrapped_session_key: wrapped,
        }).unwrap(),
    })
}

fn request_message(session_key: &str, session_id: &str, payload: Value) -> Value {
    let envelope = transport::encrypt_frame(
        session_key,
        &OuterFields {
            session_id: session_id.into(),
            route_to: "device:dev-1".into(),
        },
        &FrameFields {
            frame_type: "data".into(),
            sender: "client".into(),
            payload,
            message_id: None,
            created_at: None,
        },
        None,
    )
    .unwrap();
    json!({
        "type": "e2ee_envelope",
        "session_id": session_id,
        "envelope": serde_json::to_value(&envelope).unwrap(),
    })
}

/// A handler that reports every frame it is given as `<frame_type>:<session_id>`,
/// including the synthetic `close` a session gets when it ends.
fn reporting_handler() -> (FrameHandler, mpsc::UnboundedReceiver<String>) {
    let (seen, frames) = mpsc::unbounded_channel();
    let handler: FrameHandler = Arc::new(move |sender, frame| {
        let _ = seen.send(format!("{}:{}", frame.frame_type, sender.session_id()));
        json!({ "ok": true })
    });
    (handler, frames)
}

async fn next_frame(frames: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(10), frames.recv())
        .await
        .expect("the handler ran in time")
        .expect("channel open")
}

/// The relay socket is one carrier. When it dies, the sessions that rode nothing
/// else end with it: the app hears each session's synthetic `close`, and the same
/// session id on the next socket is a session the device no longer knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_relay_socket_ends_the_sessions_that_rode_only_it() {
    let identity = DeviceIdentity {
        device_id: "dev-1".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };
    let (handler, mut frames) = reporting_handler();
    let intake = FrameIntake::new(handler);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(16);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(16);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    let relay_socket = tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));
    let bridge = {
        let identity = identity.clone();
        let intake = intake.clone();
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
    };

    let device_transport_pub =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();
    let session_id = "sess-carried";
    let session_key = transport::generate_session_key();
    to_device_tx
        .send(session_init_message(
            session_id,
            &device_transport_pub,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device_rx).await["type"], "session_accept");
    to_device_tx
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    assert_eq!(next_frame(&mut frames).await, format!("data:{session_id}"));

    // The socket goes, with no session_closed and no client close frame.
    relay_socket.abort();
    let _ = bridge.await;

    assert_eq!(
        next_frame(&mut frames).await,
        format!("close:{session_id}"),
        "the session that rode only that socket ended with it"
    );

    // The next socket, same intake: the old session id is nobody's.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(16);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(16);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));
    let bridge = tokio::spawn(async move { relay::run(&url, &identity, intake).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
        .await
        .expect("the second socket authenticates")
        .unwrap();

    to_device_tx
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), from_device_rx.recv())
            .await
            .is_err(),
        "a frame for a session the device forgot is answered by nothing"
    );
    assert!(frames.try_recv().is_err(), "and reaches no handler");

    drop(to_device_tx);
    bridge.abort();
}

/// A `session_init` for a live session under a fresh key is the frame the spec
/// drops: no `session_accept` comes back and the session keeps the key it was
/// minted with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_init_under_a_different_key_is_refused() {
    let identity = DeviceIdentity {
        device_id: "dev-1".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(16);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(16);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));
    let (handler, _frames) = reporting_handler();
    let bridge = {
        let identity = identity.clone();
        let intake = FrameIntake::new(handler);
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
    };

    let device_transport_pub =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();
    let session_id = "sess-minted";
    let session_key = transport::generate_session_key();
    to_device_tx
        .send(session_init_message(
            session_id,
            &device_transport_pub,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device_rx).await["type"], "session_accept");

    to_device_tx
        .send(session_init_message(
            session_id,
            &device_transport_pub,
            &transport::generate_session_key(),
        ))
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), from_device_rx.recv())
            .await
            .is_err(),
        "the device accepts no session under a key that is not the one it holds"
    );

    to_device_tx
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    let response = recv(&mut from_device_rx).await;
    assert_eq!(response["type"], "e2ee_envelope");
    let envelope: Envelope = serde_json::from_value(response["envelope"].clone()).unwrap();
    transport::decrypt_envelope(&session_key, &envelope)
        .expect("the session still holds the key it was minted with");

    drop(to_device_tx);
    bridge.abort();
}

/// The carrier is released however `run` ends. A caller that drops the future
/// mid-session — a timeout around it, an abort — never reaches the read loop's
/// normal exit, and the sessions riding that socket must end all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_run_ends_the_sessions_that_rode_its_socket() {
    let identity = DeviceIdentity {
        device_id: "dev-1".into(),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };
    let (handler, mut frames) = reporting_handler();
    let intake = FrameIntake::new(handler);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws/device", listener.local_addr().unwrap());
    let (to_device_tx, to_device_rx) = mpsc::channel::<Value>(16);
    let (from_device_tx, mut from_device_rx) = mpsc::channel::<Value>(16);
    let (tkey_tx, mut tkey_rx) = mpsc::channel::<String>(1);
    tokio::spawn(mock_relay(listener, to_device_rx, from_device_tx, tkey_tx));
    let bridge = {
        let identity = identity.clone();
        let intake = intake.clone();
        tokio::spawn(async move { relay::run(&url, &identity, intake).await })
    };

    let device_transport_pub =
        tokio::time::timeout(std::time::Duration::from_secs(10), tkey_rx.recv())
            .await
            .expect("transport key arrives")
            .unwrap();
    let session_id = "sess-cut";
    let session_key = transport::generate_session_key();
    to_device_tx
        .send(session_init_message(
            session_id,
            &device_transport_pub,
            &session_key,
        ))
        .await
        .unwrap();
    assert_eq!(recv(&mut from_device_rx).await["type"], "session_accept");
    to_device_tx
        .send(request_message(
            &session_key,
            session_id,
            json!({ "method": "ping" }),
        ))
        .await
        .unwrap();
    assert_eq!(next_frame(&mut frames).await, format!("data:{session_id}"));

    // The run future is dropped mid-session, the socket still up.
    bridge.abort();
    let _ = bridge.await;

    assert_eq!(
        next_frame(&mut frames).await,
        format!("close:{session_id}"),
        "the session that rode the cancelled socket ended with it"
    );
}
