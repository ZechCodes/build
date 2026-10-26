//! Synthetic provider refusals matching #884, without reading a session store.
use super::*;

const OVERFLOW: &str = r#"{"type":"assistant","error":"invalid_request","message":{"model":"<synthetic>","content":[{"type":"text","text":"Prompt is too long"}]}}"#;
const FAILED_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":true,"duration_ms":40,"result":"Prompt is too long"}"#;

#[test]
fn a_refused_turn_retains_its_operation_and_ignores_duplicate_results() {
    let (mut reader, mut activity) = reader_and_what_it_reports();
    reader.read_line(INIT);
    {
        let mut state = reader.state.lock().unwrap();
        state.open_turn();
        state.operation_id = Some("overflow-wake".into());
    }
    reader.read_line(OVERFLOW);
    let result = r#"{"type":"result","subtype":"success","is_error":true}"#;
    reader.read_line(result);
    reader.read_line(result);
    let failures: Vec<_> = reports_already_sent(&mut activity)
        .into_iter()
        .filter_map(|report| match report.activity {
            AgentActivity::TurnFailed {
                summary,
                operation_id,
                ..
            } => Some((summary, operation_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        failures,
        vec![("Prompt is too long".into(), Some("overflow-wake".into()))]
    );
}

#[test]
fn an_acked_interruption_hands_failure_identity_to_the_steered_turn() {
    let (mut reader, mut activity) = reader_and_what_it_reports();
    reader.read_line(INIT);
    {
        let mut state = reader.state.lock().unwrap();
        state.open_turn();
        state.operation_id = Some("interrupted-operation".into());
        state.steered_operation_id = Some("steering-operation".into());
        state.pending_interrupt = Some(super::super::protocol::PendingInterrupt {
            request_id: "stop".into(),
            acked: true,
            steered: true,
        });
    }
    reader.read_line(FAILED_RESULT);
    assert!(!reports_already_sent(&mut activity)
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::TurnFailed { .. })));
    reader.read_line(OVERFLOW);
    reader.read_line(FAILED_RESULT);
    assert!(reports_already_sent(&mut activity)
        .iter()
        .any(|report| matches!(&report.activity,
        AgentActivity::TurnFailed { operation_id: Some(id), .. } if id == "steering-operation")));
}

#[test]
fn usage_limits_keep_their_reset_contract_without_a_generic_failure() {
    let (mut reader, mut activity) = reader_and_what_it_reports();
    reader.read_line(INIT);
    reader.state.lock().unwrap().open_turn();
    let mut refusal: Value = serde_json::from_str(OVERFLOW).unwrap();
    refusal["error"] = json!("rate_limit");
    reader.read_line(&refusal.to_string());
    reader.read_line(FAILED_RESULT);
    assert!(!reports_already_sent(&mut activity)
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::TurnFailed { .. })));
    reader.state.lock().unwrap().open_turn();
    reader.read_line(OVERFLOW);
    reader.read_line(FAILED_RESULT);
    assert!(reports_already_sent(&mut activity)
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::TurnFailed { .. })));
}

#[test]
fn overflow_success_subtype_is_a_failed_turn_on_every_wake() {
    let (sender, mut activity) = broadcast::channel(ACTIVITY_BACKLOG);
    let (mut reader, watched) = reader_and_its_status(Arc::new(Mutex::new(Some(sender))));
    reader.read_line(INIT);
    // A tool-heavy turn can cross 200k without another successful model call.
    // Build still has the last request's 190k reading, below its old threshold.
    let mut response: Value = serde_json::from_str(TOOL_USE).unwrap();
    response["message"]["usage"] = json!({"input_tokens":190_000});
    reader.read_line(&response.to_string());
    reader.read_line(TOOL_RESULT);
    let successes = watched.borrow().successful_response_count;
    let _ = reports_already_sent(&mut activity);
    for _ in 0..3 {
        reader.state.lock().unwrap().open_turn();
        reader.read_line(OVERFLOW);
        reader.read_line(FAILED_RESULT);
        assert_eq!(watched.borrow().successful_response_count, successes);
        assert_eq!(watched.borrow().context.unwrap().context_tokens, 190_000);
        assert_eq!(
            reader.state.lock().unwrap().reported_error.as_deref(),
            Some("Prompt is too long")
        );
        let failures = reports_already_sent(&mut activity).into_iter().filter(|report| {
            matches!(&report.activity, AgentActivity::TurnFailed { summary, .. } if summary.contains("Prompt is too long"))
        }).count();
        assert_eq!(
            failures, 1,
            "a surviving refused turn must produce attention, not silently wait"
        );
    }
}

/// This fake deliberately models BOTH possible provider responses. It proves
/// that a resumed pipe can accept /compact while over-window, not that the
/// real Claude service can summarize arbitrary over-window histories.
#[tokio::test]
async fn resumed_over_window_fake_can_compact_or_refuse_without_a_boundary() {
    for can_compact in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let script = format!(
            r#"
            printf '%s\n' '{INIT}'
            while IFS= read -r line; do
                printf '%s\n' "$line" >> "$1/input"
                case "$line" in
                    *'/compact'*) printf '%s\n' '{compact}' '{result}' ;;
                    *) printf '%s\n' '{OVERFLOW}' '{FAILED_RESULT}' ;;
                esac
            done
        "#,
            compact = if can_compact {
                COMPACT_BOUNDARY_WITH_POST_TOKENS
            } else {
                OVERFLOW
            },
            result = if can_compact { RESULT } else { FAILED_RESULT }
        );
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg(script)
            .arg("over-window-resume")
            .arg(directory.path().to_string_lossy());
        let (session, mut activity) = AdkSession::spawn(&spec, None, &adk_choice()).unwrap();
        wait_for_status(&session, AgentStatus::Waiting);
        session
            .send_turn(&Turn::new("wake already stuck session"))
            .unwrap();
        wait_for_status(&session, AgentStatus::Waiting);
        let _ = reports_already_sent(&mut activity);
        session.send_turn(&Turn::new("/compact")).unwrap();
        wait_for_status(&session, AgentStatus::Waiting);
        let reports = reports_already_sent(&mut activity);
        assert_eq!(
            reports.iter().any(|report| matches!(
                report.activity,
                AgentActivity::Compaction { completed: true }
            )),
            can_compact
        );
        let written = std::fs::read_to_string(directory.path().join("input")).unwrap();
        assert_eq!(
            written.lines().count(),
            2,
            "no automatic replay or compaction loop"
        );
        session.end();
    }
}

#[tokio::test]
async fn an_in_turn_send_attributes_terminal_failure_to_latest_operation() {
    let directory = tempfile::tempdir().unwrap();
    let spec = HarnessSpec::new("sh").arg("-c").arg(format!(
        r#"
        IFS= read -r first || exit 1
        printf '%s\n' '{INIT}' '{TOOL_USE}'
        IFS= read -r second || exit 1
        printf '%s\n' '{OVERFLOW}' '{FAILED_RESULT}'
        while IFS= read -r rest; do :; done
    "#
    ));
    let (session, mut activity) =
        AdkSession::spawn(&spec, Some(directory.path().into()), &adk_choice()).unwrap();
    session
        .send_turn(&Turn::new("first").with_operation_id("first-operation"))
        .unwrap();
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse { .. }
    ));
    session
        .send_turn(&Turn::new("in-turn send").with_operation_id("latest-operation"))
        .unwrap();
    loop {
        if let AgentActivity::TurnFailed { operation_id, .. } = next_activity(&mut activity).await {
            assert_eq!(operation_id.as_deref(), Some("latest-operation"));
            break;
        }
    }
    session.end();
}
