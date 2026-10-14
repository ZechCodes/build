use super::*;
use crate::provider_recovery::{
    abandon, episodes, save_episode, RecoveryContext, RecoveryEpisode, RecoveryJournal,
    RecoveryScope, RecoveryStage,
};
use crate::resume::disk::testing::RecordingDisk;

fn scope() -> RecoveryScope {
    RecoveryScope {
        entity_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "agent".into(),
        thread_id: "build-thread-generation".into(),
    }
}

fn journal(root: &std::path::Path) -> RecoveryJournal {
    RecoveryJournal::new(RecoveryContext {
        root: root.into(),
        scope: scope(),
    })
    .unwrap()
}

fn episode(root: &std::path::Path) -> RecoveryEpisode {
    let records = episodes(root, &scope()).unwrap();
    assert_eq!(records.len(), 1);
    records.into_iter().next().unwrap()
}

fn persist(journal: &mut RecoveryJournal, transition: &StateTransition, seconds: u64) {
    super::super::session::recovery::persist_transition(
        journal,
        transition,
        Duration::from_secs(seconds),
        "session-incarnation",
    )
    .unwrap();
}

fn working_operation() -> CodexSessionState {
    advance_to_waiting()
        .transition(
            SessionEvent::SendChosenTurn(Turn::new("go").with_operation_id("failed-operation")),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":TURN_ID}}))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
}

fn failure(state: CodexSessionState, turn: &str, seconds: u64) -> StateTransition {
    state
        .transition(
            provider_failure(turn, json!("internalServerError"), "server unavailable"),
            Duration::from_secs(seconds),
            limits().state(),
        )
        .unwrap()
}

#[test]
fn revision_621_queued_input_preserves_backoff_checkpoint_and_failed_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut journal = journal(root.path());
    let failed = failure(working_operation(), TURN_ID, 0);
    persist(&mut journal, &failed, 0);
    let before = episode(root.path());
    assert_eq!(before.stage, RecoveryStage::Pending);
    assert_eq!(before.operation_id.as_deref(), Some("failed-operation"));
    assert!(before.retry_due_ms > 0);

    let queued = failed
        .state
        .transition(
            SessionEvent::SendChosenTurn(Turn::new("next").with_operation_id("new-operation")),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    persist(&mut journal, &queued, 1);
    assert_eq!(
        episode(root.path()),
        before,
        "queued input cannot refill or discard the failure episode"
    );
}

#[test]
fn revision_621_retry_in_flight_reopens_as_one_terminal_outbox_without_new_allowance() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    let failed = failure(working_operation(), TURN_ID, 0);
    persist(&mut checkpoint, &failed, 0);
    let original = episode(root.path());
    let retry = failed
        .state
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(60),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &retry, 60);
    let mut restored = episode(root.path());
    assert_eq!(restored.stage, RecoveryStage::InFlight);
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.retry_due_ms, original.retry_due_ms);

    for _ in 0..3 {
        let _reopened = journal(root.path());
        restored = episode(root.path());
        abandon(&mut restored);
        save_episode(root.path(), &restored).unwrap();
        assert_eq!(restored.stage, RecoveryStage::Terminal);
        assert!(restored.active && restored.needs_notice);
        assert_eq!(restored.operation_id.as_deref(), Some("failed-operation"));
        assert_eq!(restored.id, original.id);
    }
    assert_eq!(
        restored
            .summary
            .matches("Provider recovery was interrupted")
            .count(),
        1
    );
}

fn starting_retry(checkpoint: &mut RecoveryJournal) -> StateTransition {
    let failed = failure(working_operation(), TURN_ID, 0);
    persist(checkpoint, &failed, 0);
    let retry = failed
        .state
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(60),
            limits().state(),
        )
        .unwrap();
    persist(checkpoint, &retry, 60);
    retry
}

fn assert_one_exhausted_episode(
    root: &std::path::Path,
    original: &RecoveryEpisode,
    operation: &str,
) {
    for _ in 0..3 {
        let mut reopened = journal(root);
        let notices = reopened.take_restored_notices();
        assert_eq!(
            notices.len(),
            1,
            "reopening must owe one final notice for the single retry episode"
        );
        let restored = episode(root);
        assert_eq!(restored.id, original.id);
        assert_eq!(restored.operation_id.as_deref(), Some(operation));
        assert_eq!(restored.retry_due_ms, original.retry_due_ms);
        assert_eq!(restored.stage, RecoveryStage::Terminal);
        assert!(restored.active && restored.needs_notice);
    }
}

#[test]
fn revision_621_retry_failure_before_start_response_keeps_one_failure_episode() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    let retry = starting_retry(&mut checkpoint);
    let original = episode(root.path());
    let [SessionEffect::Request(operation)] = retry.effects.as_slice() else {
        panic!("expected continuation")
    };
    let operation = operation.clone();
    let failed = failure(retry.state, "retry-turn", 61);
    persist(&mut checkpoint, &failed, 61);
    assert_eq!(
        episode(root.path()).id,
        original.id,
        "a second native turn is still the same single retry episode"
    );
    let answered = failed
        .state
        .transition(
            correlated(operation, Ok(json!({"turn":{"id":"retry-turn"}}))),
            Duration::from_secs(62),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &answered, 62);
    assert_one_exhausted_episode(root.path(), &original, "failed-operation");
    let later = answered
        .state
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(122),
            limits().state(),
        )
        .unwrap();
    assert!(later.effects.is_empty(), "retry allowance remains spent");
}

#[test]
fn revision_621_retry_failure_during_pending_steer_keeps_original_episode_and_latest_operation() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    let retry = starting_retry(&mut checkpoint);
    let original = episode(root.path());
    let [SessionEffect::Request(operation)] = retry.effects.as_slice() else {
        panic!("expected continuation")
    };
    let working = retry
        .state
        .transition(
            correlated(operation.clone(), Ok(json!({"turn":{"id":"retry-turn"}}))),
            Duration::from_secs(60),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &working, 60);
    let steering = working
        .state
        .transition(
            SessionEvent::SendChosenTurn(Turn::new("steer").with_operation_id("steer-operation")),
            Duration::from_secs(61),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &steering, 61);
    let failed = failure(steering.state, "retry-turn", 62);
    persist(&mut checkpoint, &failed, 62);
    assert_eq!(episode(root.path()).id, original.id);
    let answered = failed
        .state
        .transition(
            correlated(
                PendingOperation::SteerTurn {
                    thread_id: THREAD_ID.into(),
                    turn_id: "retry-turn".into(),
                    input: "steer".into(),
                },
                Ok(json!({"turnId":"retry-turn"})),
            ),
            Duration::from_secs(63),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &answered, 63);
    assert_one_exhausted_episode(root.path(), &original, "steer-operation");
}

#[test]
fn revision_621_error_before_start_response_is_durable_during_reconciliation() {
    for code in [json!("internalServerError"), json!("contextWindowExceeded")] {
        let root = tempfile::tempdir().unwrap();
        let mut checkpoint = journal(root.path());
        let starting = advance_to_waiting()
            .transition(
                SessionEvent::SendChosenTurn(Turn::new("go").with_operation_id("failed-operation")),
                Duration::ZERO,
                limits().state(),
            )
            .unwrap();
        let failed = starting
            .state
            .transition(
                provider_failure(TURN_ID, code, "provider error before response"),
                Duration::ZERO,
                limits().state(),
            )
            .unwrap();
        persist(&mut checkpoint, &failed, 0);
        let record = episode(root.path());
        assert_eq!(record.stage, RecoveryStage::Pending);
        assert_eq!(record.operation_id.as_deref(), Some("failed-operation"));
        assert!(record.active);
        assert!(!record.needs_notice);
    }
}

#[test]
fn revision_621_late_accepted_steer_updates_dispatch_identity_without_refilling_deadline() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    let steering = working_operation()
        .transition(
            SessionEvent::SendChosenTurn(Turn::new("steer").with_operation_id("steer-operation")),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    let failed = failure(steering.state, TURN_ID, 1);
    persist(&mut checkpoint, &failed, 1);
    let original = episode(root.path());
    let accepted = failed
        .state
        .transition(
            correlated(
                PendingOperation::SteerTurn {
                    thread_id: THREAD_ID.into(),
                    turn_id: TURN_ID.into(),
                    input: "steer".into(),
                },
                Ok(json!({"turnId":TURN_ID})),
            ),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &accepted, 2);
    let updated = episode(root.path());
    assert_eq!(updated.operation_id.as_deref(), Some("steer-operation"));
    assert_eq!(updated.retry_due_ms, original.retry_due_ms);
    assert_eq!(updated.stage, RecoveryStage::Pending);
    assert_eq!(updated.id, original.id);
}

#[test]
fn revision_621_exhaustion_preserves_old_outbox_when_the_next_native_turn_starts() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    let failed = failure(working_operation(), TURN_ID, 0);
    persist(&mut checkpoint, &failed, 0);
    let original = episode(root.path());
    let queued = failed
        .state
        .transition(
            SessionEvent::SendChosenTurn(Turn::new("next").with_operation_id("new-operation")),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &queued, 1);
    let retry = queued
        .state
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(60),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &retry, 60);
    let [SessionEffect::Request(operation)] = retry.effects.as_slice() else {
        panic!("expected continuation")
    };
    let retry = retry
        .state
        .transition(
            correlated(operation.clone(), Ok(json!({"turn":{"id":"retry-turn"}}))),
            Duration::from_secs(60),
            limits().state(),
        )
        .unwrap();
    persist(&mut checkpoint, &retry, 60);
    let failed = failure(retry.state, "retry-turn", 61);
    assert!(failed
        .effects
        .iter()
        .any(|effect| matches!(effect, SessionEffect::Request(_))));
    persist(&mut checkpoint, &failed, 61);
    let record = episode(root.path());
    assert_eq!(record.id, original.id);
    assert_eq!(record.operation_id.as_deref(), Some("failed-operation"));
    assert_eq!(record.stage, RecoveryStage::Terminal);
    assert!(!record.active && record.needs_notice);
    assert_eq!(
        checkpoint.last_notice_id().as_deref(),
        Some(original.id.as_str())
    );
}

#[test]
fn revision_621_failed_checkpoint_refuses_the_transition_barrier() {
    let root = tempfile::tempdir().unwrap();
    let disk = Arc::new(RecordingDisk::default());
    let mut checkpoint = RecoveryJournal::on(
        RecoveryContext {
            root: root.path().into(),
            scope: scope(),
        },
        disk.clone(),
    )
    .unwrap();
    disk.fail_next_creates(1);
    let failed = failure(working_operation(), TURN_ID, 0);
    assert!(super::super::session::recovery::persist_transition(
        &mut checkpoint,
        &failed,
        Duration::ZERO,
        "session"
    )
    .is_err());
    assert!(episodes(root.path(), &scope()).unwrap().is_empty());
}

#[test]
fn revision_621_first_checkpoint_syncs_new_directory_ancestors_and_episode() {
    let root = tempfile::tempdir().unwrap();
    let disk = Arc::new(RecordingDisk::default());
    let mut checkpoint = RecoveryJournal::on(
        RecoveryContext {
            root: root.path().into(),
            scope: scope(),
        },
        disk.clone(),
    )
    .unwrap();
    persist(
        &mut checkpoint,
        &failure(working_operation(), TURN_ID, 0),
        0,
    );
    let log = disk.log();
    assert_eq!(&log[..2], ["sync-dir", "sync-dir"]);
    assert!(log[2].starts_with("create "));
    assert!(log[3].starts_with("rename "));
    assert_eq!(log[4], "sync-dir");
}

#[test]
fn revision_621_new_session_abandons_an_old_checkpoint_instead_of_resolving_it() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    persist(
        &mut checkpoint,
        &failure(working_operation(), TURN_ID, 0),
        0,
    );
    let reopened = journal(root.path());
    assert_eq!(episode(root.path()).stage, RecoveryStage::Terminal);
    assert!(episode(root.path()).needs_notice);
    drop(reopened);
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..600 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("fake provider did not reach the expected observation");
}

fn live_session(
    root: &std::path::Path,
    suffix: &str,
) -> (
    CodexAppServerSession,
    tokio::sync::broadcast::Receiver<crate::harness::ActivityReport>,
) {
    let initialize = json!({"id":1,"result":initialize_result(&supported_user_agent())});
    let opened = json!({"id":2,"result":thread_opened_at(&root.display().to_string(), THREAD_ID, Some(SELECTED_EFFORT))});
    let goal = json!({"id":3,"result":{"goal":null}});
    let started = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
    let script = format!("read initialize; printf '%s\\n' '{initialize}'; read initialized; read thread; printf '%s\\n' '{opened}'; read goal; printf '%s\\n' '{goal}'; read turn; printf '%s\\n' '{started}'; {suffix}");
    CodexAppServerSession::spawn_with_recovery(
        &HarnessSpec::new("sh").arg("-c").arg(script),
        root.into(),
        selected_choice(),
        None,
        AppServerLimits::default(),
        Some(RecoveryContext {
            root: root.into(),
            scope: scope(),
        }),
    )
    .unwrap()
}

#[test]
fn revision_621_live_backoff_is_checkpointed_and_explicit_roll_keeps_it_pending() {
    let root = tempfile::tempdir().unwrap();
    let error = json!({"method":"error", "params":{"threadId":THREAD_ID,"turnId":TURN_ID,"willRetry":false,
        "error":{"message":"server unavailable","codexErrorInfo":"internalServerError"}}});
    let (session, mut activity) =
        live_session(root.path(), &format!("printf '%s\\n' '{error}'; read hold"));
    wait_for(|| session.status() == AgentStatus::Waiting);
    session
        .send_turn(&Turn::new("go").with_operation_id("live-failed-operation"))
        .unwrap();
    wait_for(|| {
        episodes(root.path(), &scope())
            .unwrap()
            .iter()
            .any(|record| record.stage == RecoveryStage::Pending)
    });
    let checkpoint = episode(root.path());
    assert_eq!(
        checkpoint.operation_id.as_deref(),
        Some("live-failed-operation")
    );
    assert_eq!(session.status(), AgentStatus::Working);
    session.end();
    wait_for(|| matches!(session.status(), AgentStatus::Ended { .. }));
    assert_eq!(episode(root.path()), checkpoint);
    while let Ok(report) = activity.try_recv() {
        assert!(!matches!(report.activity, AgentActivity::TurnFailed { .. }));
    }
}

#[test]
fn revision_621_unexpected_exit_checkpoints_notice_before_ended_is_observable() {
    let root = tempfile::tempdir().unwrap();
    let (session, mut activity) = live_session(root.path(), "exit 23");
    wait_for(|| session.status() == AgentStatus::Waiting);
    session
        .send_turn(&Turn::new("go").with_operation_id("live-failed-operation"))
        .unwrap();
    wait_for(|| matches!(session.status(), AgentStatus::Ended { .. }));
    let record = episode(root.path());
    assert_eq!(record.stage, RecoveryStage::Terminal);
    assert!(record.needs_notice);
    assert_eq!(
        record.operation_id.as_deref(),
        Some("live-failed-operation")
    );
    let mut report = None;
    wait_for(|| {
        if let Ok(next) = activity.try_recv() {
            if matches!(next.activity, AgentActivity::TurnFailed { .. }) {
                report = Some(next);
            }
        }
        report.is_some()
    });
    let report = report.expect("terminal failure notice");
    assert!(
        matches!(report.activity, AgentActivity::TurnFailed { episode_id: Some(id), .. } if id == record.id)
    );
}

#[test]
fn revision_621_fresh_session_reports_inherited_failure_once_without_a_boot() {
    let root = tempfile::tempdir().unwrap();
    let mut checkpoint = journal(root.path());
    persist(
        &mut checkpoint,
        &failure(working_operation(), TURN_ID, 0),
        0,
    );
    let original = episode(root.path());
    let (session, mut activity) = live_session(root.path(), "read hold");
    wait_for(|| session.status() == AgentStatus::Waiting);
    let failures = std::iter::from_fn(|| activity.try_recv().ok())
        .filter_map(|report| match report.activity {
            AgentActivity::TurnFailed {
                operation_id,
                episode_id,
                ..
            } => Some((operation_id, episode_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failures,
        vec![(Some("failed-operation".into()), Some(original.id))]
    );
    let inherited = episode(root.path());
    assert_eq!(inherited.stage, RecoveryStage::Terminal);
    assert!(inherited.active && inherited.needs_notice);
    session.end();
}
