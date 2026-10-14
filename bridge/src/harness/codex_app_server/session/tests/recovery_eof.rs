use super::*;
use crate::provider_recovery::{episodes, RecoveryScope, RecoveryStage};

/// Advance only the pure state deadline. Apply the same durable barrier and
/// outgoing effects as production, without keeping a fake provider alive 60s.
fn start_due_retry(session: &CodexAppServerSession) {
    let core = &session.core;
    let mut state = core.state.lock().unwrap();
    let retry_at = state
        .recovery_checkpoint()
        .expect("pending automatic recovery")
        .retry_at;
    let transition = state
        .transition(SessionEvent::CheckTimeouts, retry_at, core.state_limits)
        .unwrap();
    recovery::persist_transition(
        core.recovery.lock().unwrap().as_mut().unwrap(),
        &transition,
        retry_at,
        &core.recovery_source,
    )
    .unwrap();
    assert_eq!(transition.effects.len(), 1);
    for effect in &transition.effects {
        core.apply_effect(effect).unwrap();
    }
    *state = transition.state;
}

#[test]
fn revision_621_unexpected_clean_eof_during_retry_owes_one_live_terminal_notice() {
    let root = tempfile::tempdir().unwrap();
    let scope = RecoveryScope {
        entity_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "agent".into(),
        thread_id: "build-generation".into(),
    };
    let started = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
    let error = json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"willRetry":false,
        "error":{"message":"server unavailable","codexErrorInfo":"internalServerError"}}});
    let script = opened_thread_script(
        root.path(),
        &format!("read first; printf '%s\\n' '{started}' '{error}'; read retry; exit 0"),
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
    wait_until("became ready", || session.status() == AgentStatus::Waiting);
    session
        .send_turn(&Turn::new("go").with_operation_id("failed-operation"))
        .unwrap();
    wait_until("checkpointed its failure", || {
        episodes(root.path(), &scope)
            .unwrap()
            .iter()
            .any(|record| record.stage == RecoveryStage::Pending)
    });
    start_due_retry(&session);
    wait_until("ended", || {
        matches!(session.status(), AgentStatus::Ended { .. })
    });
    assert_eq!(session.status(), AgentStatus::Ended { code: Some(0) });
    assert_eq!(session.epitaph(), None);
    let reports = drain_reports(&mut activity, |_| false);
    let failures: Vec<_> = reports
        .iter()
        .filter_map(|report| match &report.activity {
            crate::harness::AgentActivity::TurnFailed {
                operation_id,
                episode_id,
                ..
            } => Some((operation_id, episode_id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "unexpected EOF must close the outstanding failure episode immediately"
    );
    let records = episodes(root.path(), &scope).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].stage, RecoveryStage::Terminal);
    assert!(records[0].needs_notice);
    assert_eq!(failures[0].0.as_deref(), Some("failed-operation"));
    assert_eq!(failures[0].1.as_deref(), Some(records[0].id.as_str()));
}
