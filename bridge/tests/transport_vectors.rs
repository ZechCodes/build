//! Common fixed fixtures, not a direct JS/Rust runtime exchange.
use build_bridge::transport::{self, Envelope, FrameFields, OuterFields, SessionInit};
use serde_json::{json, Value};

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../transport/fixtures/v1.json")).unwrap()
}

fn string<'a>(value: &'a Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

#[test]
fn pinned_identity_and_captured_bootstrap() {
    let v = vectors();
    transport::verify_transport_key_signature(
        string(&v, "identity_public"),
        string(&v, "transport_public"),
        string(&v, "signature"),
    )
    .unwrap();
    assert_eq!(
        transport::fingerprint_identity_key(string(&v, "identity_public")).unwrap(),
        string(&v, "fingerprint")
    );
    let init: SessionInit = serde_json::from_value(v["session_init"].clone()).unwrap();
    assert_eq!(
        transport::open_session_init(string(&v, "transport_private"), &init)
            .unwrap()
            .session_key_b64,
        string(&v, "session_key")
    );
    assert!(transport::open_session_init(string(&v, "wrong_transport_private"), &init).is_err());
    assert!(transport::verify_transport_key_signature(
        string(&v, "identity_public"),
        string(&v, "transport_public"),
        string(&v, "bad_signature"),
    )
    .is_err());
}

#[test]
fn captured_accept_and_frame_payloads() {
    let v = vectors();
    let key = string(&v, "session_key");
    let accept: Envelope = serde_json::from_value(v["accept"].clone()).unwrap();
    transport::verify_session_accept(key, &accept, "vector-session").unwrap();
    assert_eq!(
        serde_json::to_value(
            transport::build_session_accept(key, "vector-session", "client:fixture", Some(&accept.nonce))
                .unwrap()
        )
        .unwrap(),
        v["accept"]
    );
    assert!(transport::verify_session_accept(key, &accept, "wrong-session").is_err());
    for fixture in v["frames"].as_array().unwrap() {
        for (producer, captured) in fixture["envelopes"].as_object().unwrap() {
            let envelope: Envelope = serde_json::from_value(captured.clone()).unwrap();
            let frame = transport::decrypt_envelope(key, &envelope).unwrap();
            assert_eq!(frame.session_id, "vector-session");
            assert_eq!(frame.frame_type, string(&fixture["fields"], "frame_type"));
            assert_eq!(frame.sender, string(&fixture["fields"], "sender"));
            assert_eq!(frame.message_id, string(&fixture["fields"], "message_id"));
            assert_eq!(frame.created_at, string(&fixture["fields"], "created_at"));
            if string(fixture, "name") == "number-key-order" {
                // JSON 1.0 (Python) and 1 (JS) have distinct serde_json numbers.
                assert_eq!(frame.payload["value"].as_f64(), Some(1.0));
                assert_eq!(frame.payload["10"], "ten");
                assert_eq!(frame.payload["2"], "two");
            } else {
                assert_eq!(frame.payload, fixture["fields"]["payload"], "{producer}");
            }
        }
        let outer = OuterFields {
            session_id: string(&fixture["outer"], "session_id").into(),
            route_to: string(&fixture["outer"], "route_to").into(),
        };
        let fields = FrameFields {
            frame_type: string(&fixture["fields"], "frame_type").into(),
            sender: string(&fixture["fields"], "sender").into(),
            payload: fixture["fields"]["payload"].clone(),
            message_id: Some(string(&fixture["fields"], "message_id").into()),
            created_at: Some(string(&fixture["fields"], "created_at").into()),
        };
        let envelope = transport::encrypt_frame(key, &outer, &fields, Some(string(fixture, "nonce")))
            .unwrap();
        // Compare Rust against Python only where canonical plaintext is identical.
        assert_eq!(serde_json::to_value(envelope).unwrap(), fixture["envelopes"]["python"]);
    }
}

#[test]
fn rust_parser_baseline_and_rejections() {
    let v = vectors();
    for case in v["envelope_cases"].as_array().unwrap() {
        let decoded = serde_json::from_value::<Envelope>(case["envelope"].clone());
        let accepts = match decoded {
            Ok(env) => transport::decrypt_envelope(string(&v, "session_key"), &env).is_ok(),
            Err(_) => false,
        };
        let expected = string(case, "name") == "unknown-field";
        assert_eq!(accepts, expected, "{}", case["name"]);
    }
    // Route metadata is deliberately not authenticated by these crypto bindings.
    let mut envelope = v["frames"][0]["envelopes"]["python"].clone();
    envelope["route_to"] = json!("changed-route");
    let envelope: Envelope = serde_json::from_value(envelope).unwrap();
    transport::decrypt_envelope(string(&v, "session_key"), &envelope).unwrap();
}
