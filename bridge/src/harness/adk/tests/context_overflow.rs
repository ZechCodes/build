//! Synthetic provider refusals matching #884, without reading a session store.
use super::*;

const OVERFLOW: &str = r#"{"type":"assistant","error":"invalid_request","message":{"model":"<synthetic>","content":[{"type":"text","text":"Prompt is too long"}]}}"#;
const FAILED_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":true,"duration_ms":40,"result":"Prompt is too long"}"#;

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
