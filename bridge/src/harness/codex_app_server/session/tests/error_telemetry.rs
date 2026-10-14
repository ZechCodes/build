use super::*;

fn provider_diagnostics(info: Value) -> Vec<Value> {
    let root = tempfile::tempdir().unwrap();
    let started = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
    let error = json!({"method":"error","params":{
        "threadId":THREAD_ID, "turnId":TURN_ID, "willRetry":false,
        "error":{
            "message":r#"HTTP 503 stream disconnected {"token":"flat-json-secret","input":"private-input"}"#,
            "codexErrorInfo":info,
            "additionalDetails":"details-hidden-secret"
        }
    }});
    // The fake provider supplies the actual notification and then a protocol
    // failure, exercising telemetry through settled process shutdown.
    let script = opened_thread_script(
        root.path(),
        &format!("read turn; printf '%s\\n' '{started}' '{error}' 'not-json'; read hold"),
    );
    let (session, _activity) = scripted_session(root.path(), &script);
    wait_until("opened its thread", || {
        session.status() == AgentStatus::Waiting
    });
    session.send_turn(&Turn::new("go")).unwrap();
    wait_until("logged settled terminal telemetry", || {
        session
            .core
            .diagnostics
            .captured()
            .iter()
            .any(|record| record["event"] == "terminal_settled")
    });
    session.core.diagnostics.captured()
}

fn assert_safe_telemetry(records: &[Value], expected_code: Value) {
    for event in ["error_notification", "terminal_error", "terminal_settled"] {
        let record = records
            .iter()
            .find(|record| record["event"] == event)
            .expect(event);
        assert!(
            record.get("codex_error_info").is_some(),
            "{event}: explicit safe variant field is required"
        );
        assert_eq!(
            record["codex_error_info"], expected_code,
            "{event}: safe Codex variant must remain structured"
        );
        assert_eq!(
            record["error_code"], expected_code,
            "{event}: existing code compatibility"
        );
        assert_eq!(
            record["http_status"], 503,
            "{event}: numeric status must remain structured"
        );
        assert_eq!(
            record["error_message"], "HTTP 503 stream disconnected [REDACTED]",
            "{event}: only the human prefix is safe"
        );
        assert!(record["error_message"].as_str().unwrap().len() <= 1024);
    }
    let serialized = serde_json::to_string(records).unwrap();
    for forbidden in [
        "flat-json-secret",
        "private-input",
        "details-hidden-secret",
        "variant-hidden-secret",
        "arbitraryPayload",
        "opaqueVariantNameSecret",
        "additionalDetails",
    ] {
        assert!(
            !serialized.contains(forbidden),
            "untrusted provider field leaked: {forbidden}"
        );
    }
}

#[test]
fn round4_621_telemetry_retains_known_variant_and_status_with_only_safe_prefix() {
    let records = provider_diagnostics(json!({"responseStreamDisconnected":{
        "httpStatusCode":503, "arbitraryPayload":"variant-hidden-secret"
    }}));
    assert_safe_telemetry(&records, json!("responseStreamDisconnected"));
}

#[test]
fn round4_621_telemetry_never_renders_unknown_codex_info_payload() {
    let records = provider_diagnostics(json!({"opaqueVariantNameSecret":{
        "httpStatusCode":503, "arbitraryPayload":"variant-hidden-secret"
    }}));
    assert_safe_telemetry(&records, Value::Null);
}
