// Independent runtime modules compile before the shared-execution wiring handoff.
#[path = "../src/http/sse.rs"]
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

#[path = "../src/http/settings.rs"]
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

#[path = "../src/http/tls.rs"]
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
    let loaded = tls::load(&cert_path, &key_path).unwrap();
    assert_eq!(loaded.alpn_protocols, vec![b"http/1.1".to_vec()]);
    let other = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    std::fs::write(&key_path, other.key_pair.serialize_pem()).unwrap();
    assert!(tls::load(&cert_path, &key_path).is_err());
    std::fs::write(&key_path, vec![0u8; 65_537]).unwrap();
    assert!(tls::load(&cert_path, &key_path).is_err());
}

#[path = "../src/http/stream_slot.rs"]
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
    assert!(second_closed.load(std::sync::atomic::Ordering::SeqCst));
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

#[path = "../src/http/limits.rs"]
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
