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

#[test]
fn review621_round4_protocol_failure_keeps_useful_cause_without_provider_error() {
    let root = tempfile::tempdir().unwrap();
    let script = opened_thread_script(root.path(), "read hold");
    let (session, _activity) = scripted_session(root.path(), &script);
    wait_until("opened its thread", || {
        session.status() == AgentStatus::Waiting
    });
    session
        .core
        .apply_state(SessionEvent::TurnStarted("first-turn".into()))
        .unwrap();
    assert!(session
        .core
        .apply_state(SessionEvent::TurnStarted("unexpected-turn".into()))
        .is_err());
    wait_until("logged terminal settlement", || {
        session
            .core
            .diagnostics
            .captured()
            .iter()
            .any(|record| record["event"] == "terminal_settled")
    });
    let records = session.core.diagnostics.captured();
    for event in ["terminal_error", "terminal_settled"] {
        let record = records
            .iter()
            .find(|record| record["event"] == event)
            .unwrap();
        assert!(record["error_code"].is_null());
        assert!(record["codex_error_info"].is_null());
        assert!(record["http_status"].is_null());
        assert!(
            record["error_message"]
                .as_str()
                .unwrap()
                .contains("turn/started"),
            "protocol cause lost at {event}: {record}"
        );
    }
}

#[test]
fn round5_621_protocol_summary_has_structured_event_and_phase_without_provider_ids() {
    let root = tempfile::tempdir().unwrap();
    let script = opened_thread_script(root.path(), "read hold");
    let (session, _activity) = scripted_session(root.path(), &script);
    wait_until("opened its thread", || {
        session.status() == AgentStatus::Waiting
    });
    session
        .core
        .apply_state(SessionEvent::TurnStarted("first-turn".into()))
        .unwrap();
    assert!(session
        .core
        .apply_state(SessionEvent::TurnStarted(
            "secret-id:{\"input\":\"confidential-request\"}".into(),
        ))
        .is_err());
    wait_until("logged terminal settlement", || {
        session
            .core
            .diagnostics
            .captured()
            .iter()
            .any(|record| record["event"] == "terminal_settled")
    });
    let expected = "protocol_violation: unexpected turn/started in phase working";
    let records = session.core.diagnostics.captured();
    for event in ["terminal_error", "terminal_settled"] {
        let record = records
            .iter()
            .find(|record| record["event"] == event)
            .unwrap();
        assert_eq!(
            record["protocol_failure"],
            json!({
                "kind":"state_transition", "event":"turn/started", "phase":"working"
            })
        );
        assert_eq!(record["error_message"], expected);
        assert_eq!(record["epitaph"], expected);
    }
    assert_eq!(session.epitaph().as_deref(), Some(expected));
    let serialized = serde_json::to_string(&records).unwrap();
    for secret in ["secret-id", "confidential-request", "first-turn"] {
        assert!(
            !serialized.contains(secret),
            "protocol payload leaked: {secret}"
        );
    }
}

#[test]
fn round5_621_protocol_summary_reaches_one_failed_activity_and_durable_notice() {
    use crate::provider_recovery::{episodes, RecoveryScope, RecoveryStage};
    let root = tempfile::tempdir().unwrap();
    let scope = RecoveryScope {
        entity_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "agent".into(),
        thread_id: "build-generation".into(),
    };
    let started = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
    let script = opened_thread_script(
        root.path(),
        &format!("read turn; printf '%s\\n' '{started}'; read hold"),
    );
    let (session, mut activity) = CodexAppServerSession::spawn_with_recovery(
        &HarnessSpec::new("sh").arg("-c").arg(script),
        root.path().into(),
        selected_choice(),
        None,
        AppServerLimits::default(),
        Some(RecoveryContext {
            root: root.path().into(),
            scope: scope.clone(),
        }),
    )
    .unwrap();
    wait_until("opened its thread", || {
        session.status() == AgentStatus::Waiting
    });
    session
        .send_turn(&Turn::new("go").with_operation_id("failed-operation"))
        .unwrap();
    wait_until("accepted its turn response", || {
        session.core.state.lock().unwrap().diagnostic_phase() == "working"
    });
    assert!(session
        .core
        .apply_state(SessionEvent::TurnStarted("unexpected-turn".into()))
        .is_err());
    wait_until("ended", || {
        matches!(session.status(), AgentStatus::Ended { .. })
    });
    let reports = drain_reports(&mut activity, |_| false);
    let failures: Vec<_> = reports
        .iter()
        .filter_map(|report| match &report.activity {
            crate::harness::AgentActivity::TurnFailed {
                operation_id,
                summary,
                episode_id,
            } => Some((operation_id, summary, episode_id)),
            _ => None,
        })
        .collect();
    let expected = "protocol_violation: unexpected turn/started in phase working";
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].0.as_deref(), Some("failed-operation"));
    assert_eq!(failures[0].1, expected);
    let notices = episodes(root.path(), &scope).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].summary, expected);
    assert_eq!(notices[0].stage, RecoveryStage::Terminal);
    assert!(notices[0].needs_notice);
    assert_eq!(failures[0].2.as_deref(), Some(notices[0].id.as_str()));
    let restored = RecoveryJournal::new(RecoveryContext {
        root: root.path().into(),
        scope,
    })
    .unwrap()
    .take_restored_notices();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].summary, expected);
}

#[test]
fn round5_621_connection_failure_keeps_only_allowlisted_kind() {
    let root = tempfile::tempdir().unwrap();
    let script = opened_thread_script(
        root.path(),
        "printf '%s\\n' '{confidential-request'; read hold",
    );
    let (session, _activity) = scripted_session(root.path(), &script);
    wait_until("logged terminal settlement", || {
        session
            .core
            .diagnostics
            .captured()
            .iter()
            .any(|record| record["event"] == "terminal_settled")
    });
    let records = session.core.diagnostics.captured();
    let record = records
        .iter()
        .find(|record| record["event"] == "terminal_settled")
        .unwrap();
    assert_eq!(
        record["protocol_failure"],
        json!({"kind":"connection", "error_kind":"invalid_json"})
    );
    assert!(record["epitaph"].as_str().unwrap().contains("invalid JSON"));
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("confidential-request"));
}

#[test]
fn round5_621_event_handling_fallback_preserves_an_already_recorded_failure() {
    let root = tempfile::tempdir().unwrap();
    let script = opened_thread_script(root.path(), "read hold");
    let (session, _activity) = scripted_session(root.path(), &script);
    wait_until("opened its thread", || {
        session.status() == AgentStatus::Waiting
    });
    session.core.fail("could not checkpoint recovery".into());
    session.core.fail_protocol(ProtocolFailure::EventHandling);
    wait_until("logged terminal settlement", || {
        session
            .core
            .diagnostics
            .captured()
            .iter()
            .any(|record| record["event"] == "terminal_settled")
    });
    let records = session.core.diagnostics.captured();
    for record in records.iter().filter(|record| {
        matches!(
            record["event"].as_str(),
            Some("terminal_error" | "terminal_settled")
        )
    }) {
        assert!(
            record["protocol_failure"].is_null(),
            "fallback replaced the first failure: {record}"
        );
        assert_eq!(record["error_message"], "could not checkpoint recovery");
    }
    assert_eq!(
        session.epitaph().as_deref(),
        Some("could not checkpoint recovery")
    );
}
