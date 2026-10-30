// Independent runtime modules compile before the shared-execution wiring handoff.
#[path = "../../src/http/sse.rs"]
mod sse;

use serde_json::json;
use sse::{channel, DeliveryError, SseEvent, MAX_EVENT_BYTES, MAX_QUEUED_BYTES, MAX_QUEUED_EVENTS};

#[tokio::test]
async fn sse_inflight_output_stays_in_byte_and_item_budget() {
    let (sender, mut receiver) = channel();
    let event =
        SseEvent::json("changes", &json!({"data":"x".repeat(MAX_QUEUED_BYTES / 2)})).unwrap();
    let length = event.len();
    sender.try_send(event).unwrap();
    let inflight = receiver.next().await.unwrap();
    assert_eq!(sender.buffered(), (1, length));
    assert_eq!(
        sender.try_send(
            SseEvent::json("changes", &json!({"data":"x".repeat(MAX_QUEUED_BYTES / 2)})).unwrap()
        ),
        Err(DeliveryError::Full)
    );
    assert!(sender.is_closed());
    drop(inflight);
    let reset = receiver.next().await.unwrap();
    assert!(reset.as_str().contains("event: reset_required\n"));
    drop(reset);
    assert!(receiver.next().await.is_none());
    assert_eq!(sender.buffered(), (0, 0));
}

#[tokio::test]
async fn sse_event_overflow_explicitly_resets_and_closes() {
    let (sender, mut receiver) = channel();
    for _ in 0..MAX_QUEUED_EVENTS {
        sender
            .try_send(SseEvent::json("changes", &json!({"items":[]})).unwrap())
            .unwrap();
    }
    assert_eq!(sender.buffered().0, MAX_QUEUED_EVENTS);
    assert_eq!(
        sender.try_send(SseEvent::json("changes", &json!({})).unwrap()),
        Err(DeliveryError::Full)
    );
    let reset = receiver.next().await.unwrap();
    assert!(reset.as_str().contains("current_state_refresh"));
    drop(reset);
    assert!(receiver.next().await.is_none());
    assert_eq!(
        sender.try_send(SseEvent::json("changes", &json!({})).unwrap()),
        Err(DeliveryError::Closed)
    );
}

#[tokio::test]
async fn sse_close_discards_pending_output_and_receiver_drop_fences_producer() {
    let (sender, mut receiver) = channel();
    sender
        .try_send(SseEvent::json("changes", &json!({"private":true})).unwrap())
        .unwrap();
    sender.close();
    assert!(receiver.next().await.is_none());
    assert_eq!(sender.buffered(), (0, 0));
    let (sender, receiver) = channel();
    drop(receiver);
    assert_eq!(
        sender.try_send(SseEvent::heartbeat()),
        Err(DeliveryError::Closed)
    );
}

#[test]
fn sse_encoding_never_emits_cursor_and_rejects_oversized_or_unclassified_event() {
    let event = SseEvent::json(
        "hello",
        &json!({"durable":false,"recovery":"current_state_refresh"}),
    )
    .unwrap();
    assert_eq!(
        event.as_str(),
        "event: hello\ndata: {\"durable\":false,\"recovery\":\"current_state_refresh\"}\n\n"
    );
    assert!(!event.as_str().contains("id:"));
    assert_eq!(
        SseEvent::json("term.output", &json!({})),
        Err(DeliveryError::Unsupported)
    );
    assert_eq!(
        SseEvent::json("changes\nid: forged", &json!({})),
        Err(DeliveryError::Unsupported)
    );
    assert_eq!(
        SseEvent::json("changes", &json!({"data":"x".repeat(MAX_EVENT_BYTES)})),
        Err(DeliveryError::TooLarge)
    );
    assert_eq!(SseEvent::heartbeat().as_str(), ": keepalive\n\n");
}

#[path = "../../src/http/settings.rs"]
mod settings;

#[test]
fn http_is_disabled_by_default_and_ignores_inert_settings() {
    let values = [("BRIDGE_HTTP_TLS_KEY", "missing")];
    assert!(settings::HttpSettings::resolve(|key| values
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string()))
    .unwrap()
    .is_none());
}

#[test]
fn enabled_http_requires_explicit_bind_origin_and_tls() {
    let mut values = std::collections::HashMap::from([("BRIDGE_HTTP_ENABLED", "true")]);
    assert!(settings::HttpSettings::resolve(|key| values.get(key).map(|v| v.to_string())).is_err());
    values.insert("BRIDGE_HTTP_BIND", "127.0.0.1:9443");
    values.insert("BRIDGE_HTTP_ORIGIN", "https://bridge.example:9443");
    values.insert("BRIDGE_HTTP_TLS_CERT", "/test/cert.pem");
    values.insert("BRIDGE_HTTP_TLS_KEY", "/test/key.pem");
    let resolved = settings::HttpSettings::resolve(|key| values.get(key).map(|v| v.to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(resolved.bind, "127.0.0.1:9443".parse().unwrap());
    assert_eq!(resolved.origin, "https://bridge.example:9443");
    assert!(resolved.allowed_origins.is_none());
    values.insert("BRIDGE_HTTP_BIND", "localhost:9443");
    assert!(settings::HttpSettings::resolve(|key| values.get(key).map(|v| v.to_string())).is_err());
}

#[test]
fn http_flag_and_limits_fail_closed_on_malformed_configuration() {
    for flag in ["", "yes", "TRUE", "2"] {
        assert!(settings::HttpSettings::resolve(
            |key| (key == "BRIDGE_HTTP_ENABLED").then(|| flag.to_string())
        )
        .is_err());
    }
    assert!(settings::HttpSettings::resolve(
        |key| (key == "BRIDGE_HTTP_ENABLED").then(|| "false".to_string())
    )
    .unwrap()
    .is_none());
}

#[path = "../../src/http/tls.rs"]
mod tls;

#[test]
fn native_tls_requires_matching_key_and_owner_only_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(tls::load(&cert_path, &key_path).is_err());
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let loaded = tls::load_identity(&cert_path, &key_path).unwrap();
    assert_eq!(loaded.config.alpn_protocols, vec![b"http/1.1".to_vec()]);
    use sha2::Digest;
    let expected = sha2::Sha256::digest(cert.der().as_ref())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(loaded.certificate_fingerprint, expected);
    let other = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    std::fs::write(&key_path, other.key_pair.serialize_pem()).unwrap();
    assert!(tls::load(&cert_path, &key_path).is_err());
    std::fs::write(&key_path, vec![0u8; 65_537]).unwrap();
    assert!(tls::load(&cert_path, &key_path).is_err());
}

#[path = "../../src/http/stream_slot.rs"]
mod stream_slot;

#[derive(Clone)]
struct TestBinding(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl stream_slot::RetireBinding for TestBinding {
    fn retire(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn stream_handle_cas_keeps_tombstone_and_fences_late_calls_and_cleanup() {
    use stream_slot::{StreamError, StreamSlot};
    let slot = StreamSlot::default();
    let first = slot.replace(None).unwrap();
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    slot.bind(&first, |_| Ok(TestBinding(closed.clone())))
        .unwrap();
    assert!(slot.binding(Some(&first)).is_ok());
    assert_eq!(slot.binding(None).err(), Some(StreamError::Required));
    assert_eq!(
        slot.bind(&first, |_| Ok(TestBinding(closed.clone()))).err(),
        Some(StreamError::Consumed)
    );
    assert_eq!(slot.replace(None).err(), Some(StreamError::Replaced));
    let second = slot.replace(Some(&first)).unwrap();
    assert_eq!(
        slot.binding(Some(&second)).err(),
        Some(StreamError::Replaced)
    );
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        slot.binding(Some(&first)).err(),
        Some(StreamError::Replaced)
    );
    assert_eq!(slot.delete(&first).err(), Some(StreamError::Replaced));
    let second_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    slot.bind(&second, |_| Ok(TestBinding(second_closed.clone())))
        .unwrap();
    slot.release(&first);
    assert!(!second_closed.load(std::sync::atomic::Ordering::SeqCst));
    slot.delete(&second).unwrap();
    assert_eq!(
        slot.binding(Some(&second)).err(),
        Some(StreamError::Replaced)
    );
    assert!(second_closed.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn deleted_stream_tombstone_requires_exact_predecessor() {
    use stream_slot::{StreamError, StreamSlot};
    let slot = StreamSlot::<TestBinding>::default();
    let first = slot.replace(None).unwrap();
    let second = slot.replace(Some(&first)).unwrap();
    slot.delete(&second).unwrap();
    assert_eq!(slot.replace(None).err(), Some(StreamError::Replaced));
    assert_eq!(
        slot.replace(Some(&first)).err(),
        Some(StreamError::Replaced)
    );
    let third = slot.replace(Some(&second)).unwrap();
    assert_ne!(second, third);
    assert_eq!(
        slot.replace(Some(&second)).err(),
        Some(StreamError::Replaced)
    );
}

#[path = "../../src/http/limits.rs"]
mod limits;

#[test]
fn body_budget_reserves_complete_allowance_and_preserves_small_call_headroom() {
    let budget = limits::BufferBudget::default();
    let opening = limits::OpeningBudget::default();
    let large = budget
        .reserve(7_056_044, 8 * 1024 * 1024, &opening)
        .unwrap();
    assert!(budget
        .reserve(7_056_044, 8 * 1024 * 1024, &opening)
        .is_err());
    let other = limits::OpeningBudget::default();
    assert!(budget.reserve(7_056_044, 8 * 1024 * 1024, &other).is_err());
    let small = budget
        .reserve(1024 * 1024, 8 * 1024 * 1024, &other)
        .unwrap();
    assert!(budget.available() < limits::TOTAL_BUFFER_BYTES);
    let retained = std::sync::Arc::new(large);
    let running = retained.clone();
    drop(retained); // the HTTP future goes away; executing reply target owns it
    assert!(budget
        .reserve(7_056_044, 8 * 1024 * 1024, &opening)
        .is_err());
    drop(running);
    assert!(budget.reserve(7_056_044, 8 * 1024 * 1024, &opening).is_ok());
    drop(small);
    assert_eq!(budget.available(), limits::TOTAL_BUFFER_BYTES);
}

#[path = "../../src/http/listener.rs"]
mod listener;

#[tokio::test]
async fn native_listener_serves_tls_and_shuts_down_all_connections() {
    let dir = tempfile::tempdir().unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let config = tls::load(&cert_path, &key_path).unwrap();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let router = axum::Router::new().route("/status", axum::routing::get(|| async { "ready" }));
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(listener::serve(socket, config, router, shutdown));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    let response = client
        .get(format!("https://localhost:{}/status", address.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "ready");
    // Plaintext is never served even when HTTPS has a valid listener.
    assert!(client
        .get(format!("http://{address}/status"))
        .send()
        .await
        .is_err());
    stop.send(true).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(6), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(client
        .get(format!("https://localhost:{}/status", address.port()))
        .send()
        .await
        .is_err());
}

#[path = "../../src/http/body.rs"]
mod body;

#[tokio::test]
async fn body_limits_cover_chunked_exact_boundary_and_slow_input() {
    use axum::body::Body;
    let exact = b"{\"x\":1}";
    assert_eq!(
        body::read_json(Body::from(exact.as_slice()), exact.len())
            .await
            .unwrap(),
        json!({"x":1})
    );
    assert_eq!(
        body::read_json(Body::from(exact.as_slice()), exact.len() - 1)
            .await
            .err(),
        Some(body::BodyError::TooLarge)
    );
    let chunks = futures_util::stream::iter([
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"{\"x\":")),
        Ok(bytes::Bytes::from_static(b"1}")),
    ]);
    assert_eq!(
        body::read_json(Body::from_stream(chunks), exact.len() - 1)
            .await
            .err(),
        Some(body::BodyError::TooLarge)
    );
    assert_eq!(
        body::read_json(Body::from("invalid"), 100).await.err(),
        Some(body::BodyError::InvalidJson)
    );
}

#[tokio::test(start_paused = true)]
async fn slow_body_and_socket_stalled_write_are_independently_bounded() {
    use axum::body::Body;
    let pending = futures_util::stream::pending::<Result<bytes::Bytes, std::io::Error>>();
    assert_eq!(
        body::read_json(Body::from_stream(pending), 100).await.err(),
        Some(body::BodyError::Timeout)
    );
    let (socket, _peer) = tokio::io::duplex(1);
    let mut io = listener::BoundedIo::new(socket, listener::OutputAuthority::default());
    use tokio::io::AsyncWriteExt;
    let error = io.write_all(&[0; 2]).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn json_output_is_serialized_once_with_explicit_cap_and_reservation_lifetime() {
    let value = json!({"x":"a".repeat(100)});
    let expected = serde_json::to_vec(&value).unwrap();
    assert_eq!(body::encode_json(&value, expected.len()).unwrap(), expected);
    assert_eq!(
        body::encode_json(&value, expected.len() - 1).err(),
        Some(body::BodyError::TooLarge)
    );
    let budget = limits::BufferBudget::default();
    let permit = std::sync::Arc::new(
        budget
            .reserve(100, 100, &limits::OpeningBudget::default())
            .unwrap(),
    );
    let bytes = body::reserved_bytes(expected, permit.clone());
    drop(permit);
    let held = bytes.clone();
    drop(bytes);
    assert_eq!(budget.available(), limits::TOTAL_BUFFER_BYTES - 200);
    drop(held);
    assert_eq!(budget.available(), limits::TOTAL_BUFFER_BYTES);
}

#[path = "../../src/http/endpoint_state.rs"]
mod endpoint_state;

#[test]
fn endpoint_epoch_survives_restart_rotates_on_policy_disable_and_tracks_registration() {
    use endpoint_state::{EndpointPolicy, EndpointRecord};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("http-endpoint.json");
    let policy = EndpointPolicy {
        device_id: "device".into(),
        issuer: "https://app.example".into(),
        origin: "https://bridge.example".into(),
        allowed_origins: vec!["https://app.example".into()],
        certificate_fingerprint: "certificate".into(),
    };
    assert!(EndpointRecord::prepare(&path, None).unwrap().is_none());
    let first = EndpointRecord::prepare(&path, Some(policy.clone()))
        .unwrap()
        .unwrap();
    assert!(first.enabled);
    assert!(first.registered_epoch.is_none());
    let restarted = EndpointRecord::prepare(&path, Some(policy.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(first.endpoint_epoch, restarted.endpoint_epoch);
    first.acknowledge(&path).unwrap();
    let registered = EndpointRecord::prepare(&path, Some(policy.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        registered.registered_epoch.as_deref(),
        Some(first.endpoint_epoch.as_str())
    );
    let disabled = EndpointRecord::prepare(&path, None).unwrap().unwrap();
    assert!(!disabled.enabled);
    assert_ne!(first.endpoint_epoch, disabled.endpoint_epoch);
    assert_eq!(disabled.registered_epoch, registered.registered_epoch);
    assert_eq!(
        disabled.endpoint_epoch,
        EndpointRecord::prepare(&path, None)
            .unwrap()
            .unwrap()
            .endpoint_epoch
    );
    assert!(first.acknowledge(&path).is_err()); // delayed registration cannot overwrite successor
    disabled.acknowledge(&path).unwrap();
    let enabled = EndpointRecord::prepare(&path, Some(policy.clone()))
        .unwrap()
        .unwrap();
    assert_ne!(disabled.endpoint_epoch, enabled.endpoint_epoch);
    let mut changed = policy;
    changed.allowed_origins.push("https://other.example".into());
    let changed = EndpointRecord::prepare(&path, Some(changed))
        .unwrap()
        .unwrap();
    assert_ne!(enabled.endpoint_epoch, changed.endpoint_epoch);
    assert_eq!(
        changed.registered_epoch.as_deref(),
        Some(disabled.endpoint_epoch.as_str())
    );
}

#[path = "../../src/http/policy.rs"]
mod policy;

#[tokio::test]
async fn native_policy_checks_authority_exact_cors_and_rejects_compression() {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let router = axum::Router::new().route("/v1/test", axum::routing::get(|| async { "secret" }));
    let router = policy::HttpPolicy::new(
        "https://bridge.example",
        &["https://app.example".to_string()],
    )
    .unwrap()
    .apply(router);
    let req = |host: &str, origin: &str| {
        Request::builder()
            .uri("/v1/test")
            .header("host", host)
            .header("origin", origin)
            .body(Body::empty())
            .unwrap()
    };
    let response = router
        .clone()
        .oneshot(req("bridge.example", "https://app.example"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://app.example"
    );
    assert!(response
        .headers()
        .get("access-control-allow-credentials")
        .is_none());
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        router
            .clone()
            .oneshot(req("evil.example", "https://app.example"))
            .await
            .unwrap()
            .status(),
        StatusCode::MISDIRECTED_REQUEST
    );
    for origin in ["null", "https://app.example.evil", "https://evil.example"] {
        assert_eq!(
            router
                .clone()
                .oneshot(req("bridge.example", origin))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let compressed = Request::builder()
        .uri("/v1/test")
        .header("host", "bridge.example")
        .header("content-encoding", "gzip")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(compressed).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/v1/test")
        .header("host", "bridge.example")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,x-build-subscription",
        )
        .body(Body::empty())
        .unwrap();
    assert!(router
        .clone()
        .oneshot(preflight)
        .await
        .unwrap()
        .status()
        .is_success());
    let forbidden = Request::builder()
        .method("OPTIONS")
        .uri("/v1/test")
        .header("host", "bridge.example")
        .header("origin", "https://app.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "x-admin-secret")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.oneshot(forbidden).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[path = "../../src/http/discovery_limits.rs"]
mod discovery_limits;

#[tokio::test(start_paused = true)]
async fn public_discovery_rate_limit_is_source_and_device_bounded() {
    let limiter = discovery_limits::DiscoveryLimiter::default();
    let source = "127.0.0.1".parse().unwrap();
    for _ in 0..10 {
        assert!(limiter.admit(source));
    }
    assert!(!limiter.admit(source));
    for index in 2..7 {
        for _ in 0..10 {
            assert!(limiter.admit(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                127, 0, 0, index
            ))));
        }
    }
    assert!(!limiter.admit("127.0.0.7".parse().unwrap()));
    tokio::time::advance(std::time::Duration::from_secs(60)).await;
    assert!(limiter.admit(source));
}

#[path = "../../src/http/sse_stream.rs"]
mod sse_stream;

#[tokio::test(start_paused = true)]
async fn sse_body_holds_delivery_through_bytes_drop_and_closes_at_hard_deadline() {
    use futures_util::StreamExt;
    let (sender, receiver) = sse::channel();
    let budget = limits::BufferBudget::default();
    let reservation = std::sync::Arc::new(budget.reserve_stream().unwrap());
    let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let state = live.clone();
    let stopped = closed.clone();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    let body = sse_stream::application_body(
        sender.clone(),
        receiver,
        reservation,
        std::sync::Arc::new(move || state.load(std::sync::atomic::Ordering::SeqCst)),
        std::sync::Arc::new(move || deadline),
        std::sync::Arc::new(move || stopped.store(true, std::sync::atomic::Ordering::SeqCst)),
    );
    sender
        .try_send(SseEvent::json("changes", &json!({})).unwrap())
        .unwrap();
    let mut body = body.into_data_stream();
    let bytes = body.next().await.unwrap().unwrap();
    assert_eq!(sender.buffered().0, 1); // Hyper receiving a frame is not socket handoff
    let clone = bytes.clone();
    drop(bytes);
    assert_eq!(sender.buffered().0, 1);
    drop(clone);
    assert_eq!(sender.buffered().0, 0);
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    assert!(body.next().await.unwrap().is_err());
    drop(body);
    assert!(closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(budget.available(), limits::TOTAL_BUFFER_BYTES);
}

#[tokio::test(start_paused = true)]
async fn pending_socket_output_obeys_authority_deadline_and_exact_ticket_replacement() {
    use tokio::io::AsyncWriteExt;
    let (socket, _peer) = tokio::io::duplex(1);
    let authority = listener::OutputAuthority::default();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let old = authority.bind(
        std::sync::Arc::new(|| true),
        std::sync::Arc::new(move || deadline),
    );
    let new = authority.bind(
        std::sync::Arc::new(|| true),
        std::sync::Arc::new(move || deadline),
    );
    old.invalidate(); // cannot close current response on reused connection
    let mut io = listener::BoundedIo::new(socket, authority);
    io.write_all(&[0]).await.unwrap();
    let before = tokio::time::Instant::now();
    assert_eq!(
        io.write_all(&[1]).await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        tokio::time::Instant::now() - before,
        std::time::Duration::from_secs(2)
    );
    new.invalidate();
}

#[tokio::test]
async fn tls_stalled_reader_retains_sse_reservation_until_deadline_teardown() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tls = tls::load(&cert_path, &key_path).unwrap();
    let budget = Arc::new(limits::BufferBudget::default());
    let (sender, receiver) = sse::channel();
    let receiver = Arc::new(Mutex::new(Some(receiver)));
    let closed = Arc::new(AtomicBool::new(false));
    let (authority_tx, authority_rx) = tokio::sync::oneshot::channel();
    let authority_tx = Arc::new(Mutex::new(Some(authority_tx)));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let budget_copy = budget.clone();
    let sender_copy = sender.clone();
    let closed_copy = closed.clone();
    let router = axum::Router::new().route(
        "/events",
        axum::routing::get(
            move |axum::Extension(authority): axum::Extension<listener::OutputAuthority>| {
                let receiver = receiver.clone();
                let sender = sender_copy.clone();
                let budget = budget_copy.clone();
                let closed = closed_copy.clone();
                let authority_tx = authority_tx.clone();
                async move {
                    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
                    let _ticket = authority.bind(Arc::new(|| true), Arc::new(move || deadline));
                    authority_tx
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap()
                        .send(authority)
                        .ok();
                    sse_stream::application_body(
                        sender,
                        receiver.lock().unwrap().take().unwrap(),
                        Arc::new(budget.reserve_stream().unwrap()),
                        Arc::new(|| true),
                        Arc::new(move || deadline),
                        Arc::new(move || {
                            closed.store(true, Ordering::SeqCst);
                        }),
                    )
                }
            },
        ),
    );
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(listener::serve(socket, tls, router, shutdown));
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut client = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(
            rustls::pki_types::ServerName::try_from("localhost").unwrap(),
            socket,
        )
        .await
        .unwrap();
    client
        .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let authority = authority_rx.await.unwrap();
    let producer_sender = sender.clone();
    let producer = tokio::spawn(async move {
        let event = SseEvent::json("changes", &json!({"data":"x".repeat(262_144)})).unwrap();
        loop {
            if producer_sender.is_closed() {
                break;
            }
            if producer_sender.buffered().0 == 0 {
                let _ = producer_sender.try_send(event.clone());
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if authority.has_pending_write() {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                if authority.has_pending_write() {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reader stopped while server TLS writer is parked");
    assert!(sender.buffered().1 <= sse::MAX_QUEUED_BYTES);
    assert!(sender.buffered().0 >= 1);
    assert_eq!(
        budget.available(),
        limits::TOTAL_BUFFER_BYTES - sse::MAX_QUEUED_BYTES
    );
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !closed.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    producer.await.unwrap();
    drop(client);
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
    assert_eq!(budget.available(), limits::TOTAL_BUFFER_BYTES);
}

async fn assert_late_prior_response_ticket_cannot_retire_successor(deferred_release: bool) {
    use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc, Mutex};
    struct Released(Arc<AtomicBool>);
    impl Drop for Released {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let tls = tls::load(&cert_path, &key_path).unwrap();
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let first_tx = Arc::new(Mutex::new(Some(first_tx)));
    let (next_tx, next_rx) = tokio::sync::oneshot::channel();
    let next_tx = Arc::new(Mutex::new(Some(next_tx)));
    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(1);
    let event_rx = Arc::new(Mutex::new(Some(event_rx)));
    let router = axum::Router::new()
        .route(
            "/first",
            axum::routing::get(
                move |axum::Extension(authority): axum::Extension<listener::OutputAuthority>| {
                    let tx = first_tx.clone();
                    async move {
                        let deadline =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                        let ticket = authority.bind(Arc::new(|| true), Arc::new(move || deadline));
                        tx.lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send((authority, ticket))
                            .ok();
                        "first"
                    }
                },
            ),
        )
        .route(
            "/next",
            axum::routing::get(
                move |axum::Extension(authority): axum::Extension<listener::OutputAuthority>| {
                    let tx = next_tx.clone();
                    let rx = event_rx.clone();
                    async move {
                        let deadline =
                            tokio::time::Instant::now() + std::time::Duration::from_secs(5);
                        let _ticket = authority.bind(Arc::new(|| true), Arc::new(move || deadline));
                        tx.lock().unwrap().take().unwrap().send(authority).ok();
                        let receiver = rx.lock().unwrap().take().unwrap();
                        axum::body::Body::from_stream(futures_util::stream::unfold(
                            receiver,
                            |mut rx| async move {
                                rx.recv()
                                    .await
                                    .map(|data| (Ok::<_, std::io::Error>(data), rx))
                            },
                        ))
                    }
                },
            ),
        );
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(listener::serve(socket, tls, router, shutdown));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .pool_max_idle_per_host(1)
        .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(format!("https://localhost:{port}/first"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "first"
    );
    let (first_authority, old_ticket) = first_rx.await.unwrap();
    event_tx
        .send(bytes::Bytes::from_static(b"first event\n"))
        .await
        .unwrap();
    let mut response = client
        .get(format!("https://localhost:{port}/next"))
        .send()
        .await
        .unwrap();
    let next_authority = next_rx.await.unwrap();
    assert!(first_authority.same_connection(&next_authority));
    assert_eq!(response.chunk().await.unwrap().unwrap(), "first event\n");
    let released = Arc::new(AtomicBool::new(false));
    if deferred_release {
        old_ticket.release_with(Released(released.clone()));
        assert!(
            !released.load(Ordering::SeqCst),
            "prior response owner remains retained before a socket flush"
        );
    } else {
        old_ticket.invalidate();
    }
    event_tx
        .send(bytes::Bytes::from_static(b"successor event\n"))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "successor event\n"
    );
    if deferred_release {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !released.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the next successful flush releases the prior response owner");
    }
    drop(response);
    drop(event_tx);
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn tls_late_prior_response_ticket_cannot_retire_successor_on_same_connection() {
    assert_late_prior_response_ticket_cannot_retire_successor(false).await;
}

#[tokio::test]
async fn tls_deferred_release_with_cannot_retire_successor_on_same_connection() {
    assert_late_prior_response_ticket_cannot_retire_successor(true).await;
}

#[tokio::test(start_paused = true)]
async fn exact_output_retirement_wakes_a_parked_socket_without_waiting_for_deadline() {
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;
    let (socket, _peer) = tokio::io::duplex(1);
    let authority = listener::OutputAuthority::default();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let ticket = authority.bind(Arc::new(|| true), Arc::new(move || deadline));
    let mut io = listener::BoundedIo::new(socket, authority.clone());
    io.write_all(&[0]).await.unwrap();
    let writing = tokio::spawn(async move { io.write_all(&[1]).await });
    tokio::task::yield_now().await;
    assert!(authority.has_pending_write());
    let before = tokio::time::Instant::now();
    ticket.invalidate();
    assert_eq!(
        writing.await.unwrap().unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(before, tokio::time::Instant::now());
}

#[test]
fn concurrent_first_stream_posts_admit_only_one_and_unused_deleted_handle_cannot_bind() {
    use std::sync::{Arc, Barrier};
    let slot = Arc::new(stream_slot::StreamSlot::<TestBinding>::default());
    let start = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let slot = slot.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                slot.replace(None)
            })
        })
        .collect();
    let successful: Vec<_> = threads
        .into_iter()
        .filter_map(|thread| thread.join().unwrap().ok())
        .collect();
    assert_eq!(successful.len(), 1);
    slot.delete(&successful[0]).unwrap();
    assert_eq!(
        slot.bind(&successful[0], |_| Ok(TestBinding(Arc::new(
            std::sync::atomic::AtomicBool::new(false)
        ))))
        .err(),
        Some(stream_slot::StreamError::Consumed)
    );
}
