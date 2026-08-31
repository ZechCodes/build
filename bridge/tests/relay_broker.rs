//! Integration tests for the production relay broker (`src/bin/relay.rs`): each test
//! spawns the real binary against a wiremock api and drives real WebSockets.
//!
//! The api mocks require the `X-Internal-Secret` header, so every green auth flow is
//! also proof the relay attaches it; an unreachable api must fail closed.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream as StdTcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use build_bridge::identity::{self, StoredIdentity};
use build_bridge::relay_server::AUTH_PATH;
use build_bridge::transport;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const INTERNAL_SECRET: &str = "test-internal-secret";
const GATEWAY_TOKEN: &str = "gw_test_token";

/// The relay binary under test, bound to an ephemeral port parsed from its banner.
struct RelayProcess {
    child: Child,
    port: u16,
}

impl RelayProcess {
    fn start(api_url: &str) -> RelayProcess {
        RelayProcess::start_with(api_url, &[])
    }

    fn start_with(api_url: &str, extra_env: &[(&str, &str)]) -> RelayProcess {
        let mut command = Command::new(env!("CARGO_BIN_EXE_relay"));
        command
            .env("RELAY_PORT", "0")
            .env("API_INTERNAL_URL", api_url)
            .env("RELAY_INTERNAL_SECRET", INTERNAL_SECRET)
            .stdout(Stdio::piped());
        for (name, value) in extra_env {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("relay binary starts");
        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();
        let banner = lines
            .next()
            .expect("relay prints its listening banner")
            .expect("banner is readable");
        let port = banner
            .split("0.0.0.0:")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("unparseable relay banner: {banner}"));
        // Keep draining stdout so the child never blocks on a full pipe.
        std::thread::spawn(move || for _line in lines {});
        RelayProcess { child, port }
    }

    fn ws_url(&self, ws_path: &str) -> String {
        format!("ws://127.0.0.1:{}{ws_path}", self.port)
    }

    fn send_sigterm(&self) {
        let delivered = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(delivered.success(), "SIGTERM delivered");
    }

    fn wait_for_clean_exit(&mut self) {
        for _ in 0..100 {
            if let Some(status) = self.child.try_wait().expect("try_wait works") {
                assert!(status.success(), "relay exits cleanly, got {status}");
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("relay did not exit within 10s of SIGTERM");
    }
}

impl Drop for RelayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A mock api that accepts the gateway token and any device-status report — every
/// mock requires `X-Internal-Secret`, so passing tests prove the header is sent.
async fn mock_api() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/internal/gateway-token/{GATEWAY_TOKEN}")))
        .and(header("x-internal-secret", INTERNAL_SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"user_id": "u1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header("x-internal-secret", INTERNAL_SECRET))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

/// Register a device record with the mock api, owned by `owner_user_id`.
async fn mount_device_record(api: &MockServer, device: &StoredIdentity, owner_user_id: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/internal/devices/{}", device.device_id)))
        .and(header("x-internal-secret", INTERNAL_SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "identity_public_key_b64": device.identity_public_key_b64,
            "approved": true,
            "owner_user_id": owner_user_id,
        })))
        .mount(api)
        .await;
}

async fn connect_ws(url: &str) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connects");
    ws
}

/// Connect `/ws/client` and complete the gateway-token handshake.
async fn authed_client(relay: &RelayProcess) -> Ws {
    let mut ws = connect_ws(&relay.ws_url("/ws/client")).await;
    ws.send(Message::Text(
        json!({"type": "authenticate", "token": GATEWAY_TOKEN}).to_string(),
    ))
    .await
    .expect("authenticate frame sends");
    let reply = recv_json(&mut ws).await;
    assert_eq!(reply["type"], "authenticated");
    ws
}

/// Connect `/ws/device` with a freshly signed Ed25519 upgrade challenge and wait
/// for the relay's `authenticated` greeting.
async fn authed_device(relay: &RelayProcess, device: &StoredIdentity) -> Ws {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
        .to_string();
    let challenge = format!("{timestamp}.GET.{AUTH_PATH}");
    let signature =
        transport::sign_message_b64(&device.identity_private_key_b64, challenge.as_bytes())
            .expect("challenge signs");
    let mut request = relay
        .ws_url("/ws/device")
        .into_client_request()
        .expect("valid ws url");
    let headers = request.headers_mut();
    headers.insert(
        "X-Device-Id",
        HeaderValue::from_str(&device.device_id).unwrap(),
    );
    headers.insert("X-Timestamp", HeaderValue::from_str(&timestamp).unwrap());
    headers.insert("X-Signature", HeaderValue::from_str(&signature).unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("device connects");
    let greeting = recv_json(&mut ws).await;
    assert_eq!(greeting["type"], "authenticated");
    ws
}

async fn recv_json(ws: &mut Ws) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("frame within 10s")
            .expect("stream still open")
            .expect("frame reads");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).expect("frame is json");
        }
    }
}

/// Assert the relay ends the connection (Close frame, EOF, or reset).
async fn expect_disconnect(ws: &mut Ws) {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("disconnect within 10s")
        {
            None | Some(Ok(Message::Close(_))) | Some(Err(_)) => return,
            Some(Ok(_)) => continue,
        }
    }
}

/// Assert no ROUTED frame arrives on this socket for a beat (negative routing
/// checks). Ping/Pong keepalives are connection plumbing, not routed traffic.
async fn expect_silence(ws: &mut Ws) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return, // the window elapsed in silence
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            Ok(unexpected) => panic!("expected silence, got {unexpected:?}"),
        }
    }
}

#[tokio::test]
async fn health_endpoint_answers_plain_http_200_without_auth() {
    let api = mock_api().await;
    let relay = RelayProcess::start(&api.uri());

    let mut tcp = StdTcpStream::connect(("127.0.0.1", relay.port)).expect("tcp connects");
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.write_all(b"GET /health HTTP/1.1\r\nHost: relay\r\n\r\n")
        .unwrap();
    let mut status_line = String::new();
    BufReader::new(tcp).read_line(&mut status_line).unwrap();
    assert!(
        status_line.starts_with("HTTP/1.1 200"),
        "health probe answered 200, got: {status_line:?}"
    );
}

#[tokio::test]
async fn client_authenticates_and_internal_calls_carry_the_secret_header() {
    let api = mock_api().await;
    let relay = RelayProcess::start(&api.uri());
    // mock_api only matches requests bearing X-Internal-Secret, so success here
    // proves the relay sent it.
    let _client = authed_client(&relay).await;
}

#[tokio::test]
async fn client_auth_fails_closed_when_api_is_unreachable() {
    // Nothing listens on port 9 — every internal lookup errors, so auth must refuse.
    let relay = RelayProcess::start("http://127.0.0.1:9");
    let mut ws = connect_ws(&relay.ws_url("/ws/client")).await;
    ws.send(Message::Text(
        json!({"type": "authenticate", "token": GATEWAY_TOKEN}).to_string(),
    ))
    .await
    .unwrap();
    expect_disconnect(&mut ws).await;
}

#[tokio::test]
async fn unauthenticated_client_gets_exactly_one_frame() {
    let api = mock_api().await;
    let relay = RelayProcess::start(&api.uri());

    // A first frame that is not `authenticate` closes the connection.
    let mut wrong_type = connect_ws(&relay.ws_url("/ws/client")).await;
    wrong_type
        .send(Message::Text(json!({"type": "hello"}).to_string()))
        .await
        .unwrap();
    expect_disconnect(&mut wrong_type).await;

    // So does a non-text first frame — no free pre-auth traffic.
    let mut binary_first = connect_ws(&relay.ws_url("/ws/client")).await;
    binary_first
        .send(Message::Binary(vec![0x42; 16]))
        .await
        .unwrap();
    expect_disconnect(&mut binary_first).await;
}

#[tokio::test]
async fn oversized_frames_close_the_connection() {
    let api = mock_api().await;
    let relay = RelayProcess::start(&api.uri());
    let mut ws = authed_client(&relay).await;
    // Just past the relay's MAX_WS_MESSAGE_BYTES cap (8 MiB). The relay may reset
    // the connection while we are still writing — that send error is also a pass.
    let oversized = "x".repeat(9 * 1024 * 1024);
    if ws.send(Message::Text(oversized)).await.is_ok() {
        expect_disconnect(&mut ws).await;
    }
}

#[tokio::test]
async fn device_online_and_offline_are_pushed_to_the_owners_clients() {
    let api = mock_api().await;
    let device = identity::generate("laptop");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start(&api.uri());

    let mut client = authed_client(&relay).await;
    let mut device_ws = authed_device(&relay, &device).await;

    let online = recv_json(&mut client).await;
    assert_eq!(online["type"], "device_online");
    assert_eq!(online["device_id"], device.device_id.as_str());

    device_ws.close(None).await.unwrap();
    let offline = recv_json(&mut client).await;
    assert_eq!(offline["type"], "device_offline");
    assert_eq!(offline["device_id"], device.device_id.as_str());
}

#[tokio::test]
async fn one_client_sessions_to_multiple_devices_and_ownership_is_enforced() {
    let api = mock_api().await;
    let device_a = identity::generate("dev-a");
    let device_b = identity::generate("dev-b");
    let foreign_device = identity::generate("foreign");
    mount_device_record(&api, &device_a, "u1").await;
    mount_device_record(&api, &device_b, "u1").await;
    mount_device_record(&api, &foreign_device, "someone-else").await;
    let relay = RelayProcess::start(&api.uri());

    let mut client = authed_client(&relay).await;
    let mut ws_a = authed_device(&relay, &device_a).await;
    let mut ws_b = authed_device(&relay, &device_b).await;
    let mut ws_foreign = authed_device(&relay, &foreign_device).await;

    // u1's client hears about its own devices coming online — never the foreign one.
    for _ in 0..2 {
        let online = recv_json(&mut client).await;
        assert_eq!(online["type"], "device_online");
        assert_ne!(online["device_id"], foreign_device.device_id.as_str());
    }

    // Open concurrent sessions to both owned devices via the contract's route_to.
    for (session_id, device) in [("s-a", &device_a), ("s-b", &device_b)] {
        client
            .send(Message::Text(
                json!({
                    "type": "session_init",
                    "session_id": session_id,
                    "route_to": format!("device:{}", device.device_id),
                    "session_init": {"device_id": device.device_id},
                })
                .to_string(),
            ))
            .await
            .unwrap();
    }
    assert_eq!(recv_json(&mut ws_a).await["session_id"], "s-a");
    assert_eq!(recv_json(&mut ws_b).await["session_id"], "s-b");

    // Frames route per-session in both directions, concurrently.
    ws_a.send(Message::Text(
        json!({"type": "session_accept", "session_id": "s-a"}).to_string(),
    ))
    .await
    .unwrap();
    ws_b.send(Message::Text(
        json!({"type": "session_accept", "session_id": "s-b"}).to_string(),
    ))
    .await
    .unwrap();
    let accept_one = recv_json(&mut client).await;
    let accept_two = recv_json(&mut client).await;
    let mut accepted: Vec<&str> = vec![
        accept_one["session_id"].as_str().unwrap(),
        accept_two["session_id"].as_str().unwrap(),
    ];
    accepted.sort_unstable();
    assert_eq!(accepted, ["s-a", "s-b"]);

    client
        .send(Message::Text(
            json!({"type": "e2ee_envelope", "session_id": "s-b", "payload": "opaque"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(recv_json(&mut ws_b).await["session_id"], "s-b");
    expect_silence(&mut ws_a).await;

    // A session_init routed at another user's device is dropped, not forwarded.
    client
        .send(Message::Text(
            json!({
                "type": "session_init",
                "session_id": "s-evil",
                "route_to": format!("device:{}", foreign_device.device_id),
                "session_init": {"device_id": foreign_device.device_id},
            })
            .to_string(),
        ))
        .await
        .unwrap();
    expect_silence(&mut ws_foreign).await;
}

#[tokio::test]
async fn client_disconnect_sends_session_closed_to_the_device() {
    let api = mock_api().await;
    let device = identity::generate("laptop");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start(&api.uri());

    let mut client = authed_client(&relay).await;
    let mut device_ws = authed_device(&relay, &device).await;
    assert_eq!(recv_json(&mut client).await["type"], "device_online");

    client
        .send(Message::Text(
            json!({
                "type": "session_init",
                "session_id": "s-gone",
                "route_to": format!("device:{}", device.device_id),
                "session_init": {"device_id": device.device_id},
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(recv_json(&mut device_ws).await["session_id"], "s-gone");

    // The browser goes away: the device must be told the session is dead, so it
    // stops encrypting output into it and drops the session key.
    client.close(None).await.unwrap();
    let notice = recv_json(&mut device_ws).await;
    assert_eq!(notice["type"], "session_closed");
    assert_eq!(notice["session_id"], "s-gone");
}

#[tokio::test]
async fn silent_device_is_severed_and_reported_offline() {
    // The device promised a heartbeat every HEARTBEAT_INTERVAL_S when it
    // authenticated. One that goes completely silent — a wedged bridge whose
    // event loop stopped reading and writing — must be severed at the liveness
    // deadline and reported offline, not stay registered forever while every
    // frame routed to it disappears.
    let api = mock_api().await;
    let device = identity::generate("wedged");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start_with(&api.uri(), &[("RELAY_DEVICE_LIVENESS_S", "2")]);

    let mut client = authed_client(&relay).await;
    let mut device_ws = authed_device(&relay, &device).await;
    assert_eq!(recv_json(&mut client).await["type"], "device_online");

    // The device sends nothing at all. The relay must cut it loose…
    expect_disconnect(&mut device_ws).await;
    // …and tell the owner's browsers the truth instead of leaving them waiting.
    let offline = recv_json(&mut client).await;
    assert_eq!(offline["type"], "device_offline");
    assert_eq!(offline["device_id"], device.device_id.as_str());
}

#[tokio::test]
async fn heartbeating_and_reading_device_outlives_the_liveness_deadline() {
    let api = mock_api().await;
    let device = identity::generate("healthy");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start_with(&api.uri(), &[("RELAY_DEVICE_LIVENESS_S", "2")]);

    let mut client = authed_client(&relay).await;
    let device_ws = authed_device(&relay, &device).await;
    assert_eq!(recv_json(&mut client).await["type"], "device_online");

    // A healthy device both writes (heartbeats) and reads — reading is what lets
    // the WebSocket library answer the relay's pings. Give each half its own
    // task, as the bridge does, so neither half waits on the other: taking turns
    // on one task spent the whole liveness window on a loaded machine and the
    // relay severed a device this test calls healthy.
    let (mut sink, mut source) = device_ws.split();
    let reading = tokio::spawn(async move {
        while let Some(Ok(message)) = source.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
        // Reading ends only when the relay severs the device.
    });
    let heartbeating = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if sink
                .send(Message::Text(json!({"type": "heartbeat"}).to_string()))
                .await
                .is_err()
            {
                return; // severed: the assertion below reports it
            }
        }
    });

    // Keep both halves alive well past several liveness windows.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!reading.is_finished(), "relay severed a healthy device");
    // …and the owner's browsers were never told the device went offline.
    expect_silence(&mut client).await;

    reading.abort();
    heartbeating.abort();
}

#[tokio::test]
async fn device_that_heartbeats_but_never_reads_is_severed() {
    // The 2026-08-13 wedge shape: the bridge's heartbeat task kept writing while
    // its read loop was stuck grinding, so the socket filled with unread frames
    // and every browser hung on "Waiting for your device". Heartbeats alone must
    // not count as liveness — only answering the relay's pings proves the read
    // loop is alive, and a device that writes without ever reading must be
    // severed and reported offline.
    let api = mock_api().await;
    let device = identity::generate("write-only");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start_with(&api.uri(), &[("RELAY_DEVICE_LIVENESS_S", "1")]);

    let mut client = authed_client(&relay).await;
    let device_ws = authed_device(&relay, &device).await;
    assert_eq!(recv_json(&mut client).await["type"], "device_online");

    // Split the socket: write heartbeats forever, never poll the read half —
    // so the relay's pings are never answered.
    let (mut sink, stream) = device_ws.split();
    std::mem::forget(stream);
    let heartbeats = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if sink
                .send(Message::Text(json!({"type": "heartbeat"}).to_string()))
                .await
                .is_err()
            {
                return; // severed: exactly what the test expects
            }
        }
    });

    let offline = recv_json(&mut client).await;
    assert_eq!(offline["type"], "device_offline");
    assert_eq!(offline["device_id"], device.device_id.as_str());
    heartbeats.abort();
}

#[tokio::test]
async fn device_that_stops_reading_is_severed_and_reported_offline() {
    // The incident shape: the device's socket stays open but it stops draining
    // its receive buffer. Relay writes back up, a write stalls past the limit —
    // the device must then be fully deregistered and browsers told it's offline.
    // Before the fix the stalled writer died alone, leaving the device
    // registered and every frame routed to it silently dropped.
    let api = mock_api().await;
    let device = identity::generate("deaf");
    mount_device_record(&api, &device, "u1").await;
    let relay = RelayProcess::start_with(
        &api.uri(),
        &[
            ("RELAY_WRITE_STALL_S", "1"),
            // Liveness alone must not be what severs this connection.
            ("RELAY_DEVICE_LIVENESS_S", "600"),
        ],
    );

    let mut client = authed_client(&relay).await;
    let device_ws = authed_device(&relay, &device).await;
    assert_eq!(recv_json(&mut client).await["type"], "device_online");

    client
        .send(Message::Text(
            json!({
                "type": "session_init",
                "session_id": "s-flood",
                "route_to": format!("device:{}", device.device_id),
                "session_init": {"device_id": device.device_id},
            })
            .to_string(),
        ))
        .await
        .unwrap();

    // Stop reading from the device socket entirely (drop polls it no further),
    // then flood frames at it until kernel buffers fill and a relay write stalls.
    std::mem::forget(device_ws);
    let payload = "x".repeat(1024 * 1024);
    for _ in 0..24 {
        let frame = json!({
            "type": "e2ee_envelope",
            "session_id": "s-flood",
            "payload": payload,
        })
        .to_string();
        if client.send(Message::Text(frame)).await.is_err() {
            break; // the relay may drop us once the device is severed — fine
        }
    }

    // The stalled write must sever the device and push device_offline.
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), client.next())
            .await
            .expect("device_offline within 30s of the stall")
            .expect("client connection stays open")
            .expect("frame reads");
        if let Message::Text(text) = frame {
            let msg: Value = serde_json::from_str(&text).expect("frame is json");
            if msg["type"] == "device_offline" {
                assert_eq!(msg["device_id"], device.device_id.as_str());
                break;
            }
        }
    }
}

#[tokio::test]
async fn sigterm_closes_websockets_cleanly_and_exits_zero() {
    let api = mock_api().await;
    let mut relay = RelayProcess::start(&api.uri());
    let mut client = authed_client(&relay).await;

    relay.send_sigterm();

    // The relay says goodbye with a proper Close frame — not a dropped TCP stream.
    loop {
        match tokio::time::timeout(Duration::from_secs(10), client.next())
            .await
            .expect("close within 10s")
        {
            Some(Ok(Message::Close(_))) => break,
            Some(Ok(_)) => continue,
            other => panic!("expected a clean Close frame, got {other:?}"),
        }
    }
    relay.wait_for_clean_exit();
}
