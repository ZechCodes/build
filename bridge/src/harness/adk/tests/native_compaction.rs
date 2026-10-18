//! Native compaction is activity within a live turn, not a result boundary.
use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity, PolicySession};

struct EndingSession(Arc<dyn AgentSession>);

impl Drop for EndingSession {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// Each file releases a protocol stage. The second user message stays in
/// stdin while the first turn compacts, then the same child consumes it.
fn compacting_child(directory: &Path) -> HarnessSpec {
    HarnessSpec::new("sh")
        .arg("-c")
        .arg(format!(
            r#"
            IFS= read -r line || exit 1
            printf '%s\n' "$line" > "$1/stdin.jsonl"
            printf '%s\n' '{INIT}' '{TOOL_USE}'
            while [ ! -f "$1/compact" ]; do sleep 0.01; done
            printf '%s\n' '{{"type":"system","subtype":"status","status":"compacting"}}' '{{"type":"system","subtype":"status","status":null,"compact_result":"success"}}' '{COMPACT_BOUNDARY}'
            while [ ! -f "$1/finish" ]; do sleep 0.01; done
            printf '%s\n' '{TOOL_RESULT}' '{RESULT}'
            while IFS= read -r line; do
                printf '%s\n' "$line" >> "$1/stdin.jsonl"
                printf '%s\n' '{NARRATION}' '{RESULT}'
            done
            "#
        ))
        .arg("native-compaction-fixture")
        .arg(directory.to_string_lossy())
}

#[tokio::test]
async fn native_compaction_preserves_the_live_turn_and_delivers_queued_input_once() {
    let directory = tempfile::tempdir().unwrap();
    let (session, mut activity) =
        AdkSession::spawn(&compacting_child(directory.path()), None, &adk_choice()).unwrap();
    let mut policy = AdkHarness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens: 150_000,
                source: CompactionSource::Device,
            },
            adk_choice(),
            None,
            CompactionLaunchIdentity::new("native-compaction-test"),
        )
        .unwrap();
    // Exercise native lifecycle independently of version qualification. A
    // real launch may select Native only when its adapter has evidence.
    policy.enforcement = CompactionEnforcement::Native;
    let session = EndingSession(PolicySession::attach(Arc::new(session), policy));
    session.0.send_turn(&Turn::new("long tool loop")).unwrap();
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::ToolUse { .. }
    ));
    let session_id = session.0.session_id();
    let policy_before = session.0.compaction_policy().unwrap().clone();
    assert_eq!(session_id.as_deref(), Some("sess-adk"));

    session.0.send_turn(&Turn::new("queued follow-up")).unwrap();
    std::fs::write(directory.path().join("compact"), "").unwrap();
    for completed in [false, true] {
        assert_eq!(
            next_activity(&mut activity).await,
            AgentActivity::Compaction { completed }
        );
        assert_eq!(session.0.status(), AgentStatus::Working);
        assert!(
            session.0.can_interrupt(),
            "compaction does not end the turn"
        );
        assert_eq!(session.0.session_id(), session_id);
        assert_eq!(session.0.compaction_policy(), Some(&policy_before));
        assert!(
            !session
                .0
                .compaction_policy()
                .unwrap()
                .automatic_due(Some(900_000)),
            "Build must not inject a threshold command into native compaction"
        );
    }
    assert!(activity.try_recv().is_err(), "the open tool stays open");

    std::fs::write(directory.path().join("finish"), "").unwrap();
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::ToolResult {
            outcome: ToolOutcome::Ok,
            ..
        }
    ));
    assert!(matches!(
        next_activity(&mut activity).await,
        AgentActivity::Narration { .. }
    ));
    assert!(becomes_true_within(Duration::from_secs(5), || {
        session.0.status() == AgentStatus::Waiting
    }));
    let written = std::fs::read_to_string(directory.path().join("stdin.jsonl")).unwrap();
    let turns: Vec<Value> = written
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(turns.len(), 2, "no duplicate input or /compact: {turns:?}");
    assert_eq!(turns[0]["message"]["content"][0]["text"], "long tool loop");
    assert_eq!(
        turns[1]["message"]["content"][0]["text"],
        "queued follow-up"
    );
    assert_eq!(session.0.session_id(), session_id);
    assert_eq!(session.0.epitaph(), None);
}

#[test]
fn native_status_and_precompact_hook_report_one_start_per_compaction() {
    for starts in [["status", "hook_started"], ["hook_started", "status"]] {
        let (mut reader, mut activity) = reader_and_what_it_reports();
        for _ in 0..2 {
            for subtype in starts {
                reader.read_line(
                    &json!({
                        "type": "system", "subtype": subtype,
                        "status": "compacting", "hook_event": "PreCompact",
                    })
                    .to_string(),
                );
            }
            reader.read_line(
                r#"{"type":"system","subtype":"status","status":null,"compact_result":"success"}"#,
            );
            reader.read_line(COMPACT_BOUNDARY);
        }
        let reports = reports_already_sent(&mut activity);
        assert_eq!(
            reports
                .into_iter()
                .map(|report| report.activity)
                .collect::<Vec<_>>(),
            vec![
                AgentActivity::Compaction { completed: false },
                AgentActivity::Compaction { completed: true },
                AgentActivity::Compaction { completed: false },
                AgentActivity::Compaction { completed: true },
            ],
        );
    }
}

#[test]
fn failed_native_compaction_status_never_reports_success_or_clears_context() {
    let (sender, mut activity) = broadcast::channel(ACTIVITY_BACKLOG);
    let (mut reader, watched) = reader_and_its_status(Arc::new(Mutex::new(Some(sender))));
    reader.read_line(NARRATION_WITH_USAGE);
    reader.read_line(RESULT_WITH_USAGE);
    {
        let mut state = reader.state.lock().unwrap();
        state.announced = true;
        state.open_turn();
    }
    let _ = reports_already_sent(&mut activity);
    for result in ["failed", "error", "cancelled", "interrupted"] {
        reader.read_line(r#"{"type":"system","subtype":"status","status":"compacting"}"#);
        reader.read_line(
            &json!({
                "type": "system", "subtype": "status",
                "status": null, "compact_result": result,
            })
            .to_string(),
        );
        let reports = reports_already_sent(&mut activity);
        assert_eq!(
            reports
                .into_iter()
                .map(|report| report.activity)
                .collect::<Vec<_>>(),
            vec![AgentActivity::Compaction { completed: false }],
            "{result} is not a completed compaction",
        );
        assert_eq!(watched.borrow().context.unwrap().context_tokens, 33_000);
        assert_eq!(watched.borrow().status, AgentStatus::Working);
    }
}
