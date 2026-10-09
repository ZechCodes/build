//! `/compact` as Codex's own operation, and the context the thread reports.

use super::*;
use crate::harness::TurnContext;

fn compact_thread() -> PendingOperation {
    PendingOperation::CompactThread {
        thread_id: THREAD_ID.to_string(),
    }
}

fn apply(state: CodexSessionState, event: SessionEvent) -> StateTransition {
    state
        .transition(event, Duration::ZERO, limits().state())
        .unwrap()
}

fn compacting() -> CodexSessionState {
    let sent = apply(
        advance_to_waiting(),
        SessionEvent::SendTurn("/compact".to_string()),
    );
    assert_eq!(sent.effects, vec![SessionEffect::Request(compact_thread())]);
    assert_eq!(sent.state.live_status(), Some(AgentStatus::Working));
    sent.state
}

fn compaction_answered() -> SessionEvent {
    correlated(compact_thread(), Ok(json!({})))
}

#[test]
fn a_compact_prompt_asks_codex_to_compact_the_thread_instead_of_starting_a_turn() {
    let sent = apply(
        advance_to_waiting(),
        SessionEvent::SendTurn("/compact\n\nBuild conversation protocol:\nreply".to_string()),
    );

    assert_eq!(sent.effects, vec![SessionEffect::Request(compact_thread())]);
    let mut frame = Vec::new();
    compact_thread().serialize_request(7, &mut frame).unwrap();
    let frame: Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(frame["method"], "thread/compact/start");
    assert_eq!(frame["params"], json!({"threadId": THREAD_ID}));
}

#[test]
fn a_compaction_answered_before_its_turn_works_until_the_turn_completes() {
    let answered = apply(compacting(), compaction_answered()).state;
    assert_eq!(answered.live_status(), Some(AgentStatus::Working));

    let started = apply(answered, SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    assert_eq!(started.live_status(), Some(AgentStatus::Working));

    let completed = apply(started, turn_completed(TURN_ID, None));
    assert_eq!(completed.state.live_status(), Some(AgentStatus::Waiting));
}

#[test]
fn a_compaction_completed_before_its_answer_waits_for_the_answer() {
    let started = apply(compacting(), SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    let completed = apply(started, turn_completed(TURN_ID, None)).state;
    assert_eq!(completed.live_status(), Some(AgentStatus::Working));

    let answered = apply(completed, compaction_answered());
    assert_eq!(answered.state.live_status(), Some(AgentStatus::Waiting));
}

#[test]
fn a_compaction_whose_turn_completes_after_its_answer_closes_that_turn() {
    let answered = apply(compacting(), compaction_answered()).state;
    let completed = apply(answered, turn_completed(TURN_ID, None));

    assert_eq!(completed.state.live_status(), Some(AgentStatus::Waiting));
    assert!(completed
        .effects
        .contains(&SessionEffect::CloseTurn(TURN_ID.to_string())));
}

#[test]
fn a_refused_compaction_is_reported_and_leaves_the_session_waiting() {
    let refused = apply(
        compacting(),
        correlated(
            compact_thread(),
            Err(RpcError {
                code: -32600,
                message: "thread is busy".to_string(),
                data: None,
            }),
        ),
    );

    assert_eq!(refused.state.live_status(), Some(AgentStatus::Waiting));
    assert!(matches!(
        refused.effects.as_slice(),
        [SessionEffect::Report(report)] if matches!(
            &report.activity,
            AgentActivity::TaskUpdate { summary } if summary.contains("thread is busy")
        )
    ));
}

#[test]
fn a_compact_prompt_is_never_steered_into_a_running_turn() {
    let working = apply(
        working_state(),
        SessionEvent::SendTurn("/compact".to_string()),
    );
    assert!(working.effects.is_empty());

    let completed = apply(working.state, turn_completed(TURN_ID, None));
    assert!(completed
        .effects
        .contains(&SessionEffect::Request(compact_thread())));
}

#[test]
fn token_usage_decodes_to_the_last_requests_context_and_the_threads_cache_reads() {
    let breakdown = |input: u64, cached: u64| {
        json!({
            "inputTokens": input,
            "cachedInputTokens": cached,
            "cacheWriteInputTokens": 0,
            "outputTokens": 10,
            "reasoningOutputTokens": 4,
            "totalTokens": input + 14,
        })
    };
    let params = json!({
        "threadId": THREAD_ID,
        "turnId": TURN_ID,
        "tokenUsage": {
            "last": breakdown(52_000, 48_000),
            "total": breakdown(140_000, 120_000),
            "modelContextWindow": 272_000,
        },
    });

    assert_eq!(
        ServerNotification::decode("thread/tokenUsage/updated", params).unwrap(),
        ServerNotification::TokenUsage(TurnContext {
            context_tokens: 52_000,
            cache_read_tokens: 120_000,
        })
    );
}

#[test]
fn the_compacted_notification_is_decoded() {
    let params = json!({"threadId": THREAD_ID, "turnId": TURN_ID});

    assert_eq!(
        ServerNotification::decode("thread/compacted", params).unwrap(),
        ServerNotification::ContextCompacted
    );
}

const EARLIER_TURN_ID: &str = "turn-before-compaction";
const CODEX_TURN_ID: &str = "turn-codex-started";

/// Waiting after an ordinary turn and then a `/compact`, the way #421's agent
/// was when Codex spoke again nine minutes later.
fn waiting_after_compaction() -> CodexSessionState {
    let starting = apply(
        advance_to_waiting(),
        SessionEvent::SendTurn("go".to_string()),
    )
    .state;
    let working = apply(
        starting,
        correlated(start_turn("go"), Ok(json!({"turn":{"id":EARLIER_TURN_ID}}))),
    )
    .state;
    let waiting = apply(working, turn_completed(EARLIER_TURN_ID, None)).state;
    let compacting = apply(waiting, SessionEvent::SendTurn("/compact".to_string())).state;
    let answered = apply(compacting, compaction_answered()).state;
    let started = apply(answered, SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    let completed = apply(started, turn_completed(TURN_ID, None)).state;
    assert_eq!(completed.live_status(), Some(AgentStatus::Waiting));
    completed
}

#[test]
fn a_turn_codex_starts_on_its_own_after_a_compaction_is_worked_until_it_completes() {
    let started = apply(
        waiting_after_compaction(),
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    );
    assert!(started.effects.is_empty());
    assert_eq!(started.state.live_status(), Some(AgentStatus::Working));

    let completed = apply(started.state, turn_completed(CODEX_TURN_ID, None));
    assert_eq!(completed.state.live_status(), Some(AgentStatus::Waiting));
    assert!(completed
        .effects
        .contains(&SessionEffect::CloseTurn(CODEX_TURN_ID.to_string())));
}

#[test]
fn a_message_sent_into_a_turn_codex_started_on_its_own_steers_into_it() {
    let started = apply(
        waiting_after_compaction(),
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    )
    .state;

    let sent = apply(started, SessionEvent::SendTurn("more".to_string()));
    assert_eq!(
        sent.effects,
        vec![SessionEffect::Request(PendingOperation::SteerTurn {
            thread_id: THREAD_ID.to_string(),
            turn_id: CODEX_TURN_ID.to_string(),
            input: "more".to_string(),
        })]
    );
}

#[test]
fn a_late_item_from_an_earlier_turn_leaves_the_waiting_session_waiting() {
    for turn_id in [EARLIER_TURN_ID, TURN_ID, "turn-before-this-process"] {
        let late = apply(
            waiting_after_compaction(),
            SessionEvent::ItemObserved(turn_id.to_string()),
        );
        assert!(late.effects.is_empty());
        assert_eq!(late.state.live_status(), Some(AgentStatus::Waiting));
    }
}

#[test]
fn a_late_item_from_an_earlier_turn_never_becomes_the_next_turn() {
    let starting = apply(
        waiting_after_compaction(),
        SessionEvent::SendTurn("next".to_string()),
    )
    .state;
    let late = apply(
        starting,
        SessionEvent::ItemObserved(EARLIER_TURN_ID.to_string()),
    )
    .state;

    let answered = apply(
        late,
        correlated(start_turn("next"), Ok(json!({"turn":{"id":CODEX_TURN_ID}}))),
    );
    assert_eq!(answered.state.live_status(), Some(AgentStatus::Working));
    let late_again = apply(
        answered.state,
        SessionEvent::ItemObserved(EARLIER_TURN_ID.to_string()),
    );
    assert_eq!(late_again.state.live_status(), Some(AgentStatus::Working));
    let completed = apply(late_again.state, turn_completed(CODEX_TURN_ID, None));
    assert_eq!(completed.state.live_status(), Some(AgentStatus::Waiting));
}

#[test]
fn a_message_sent_during_a_compaction_turn_waits_for_a_turn_of_its_own() {
    let answered = apply(compacting(), compaction_answered()).state;
    let compacting_turn = apply(answered, SessionEvent::TurnStarted(TURN_ID.to_string())).state;

    let sent = apply(compacting_turn, SessionEvent::SendTurn("after".to_string()));
    assert!(sent.effects.is_empty());
    assert_eq!(sent.state.queued_turn_count(), 1);

    let completed = apply(sent.state, turn_completed(TURN_ID, None));
    assert!(completed
        .effects
        .contains(&SessionEffect::Request(start_turn("after"))));
    assert_eq!(completed.state.queued_turn_count(), 0);
}

#[test]
fn a_message_sent_while_a_compaction_turn_is_announced_before_its_answer_is_queued() {
    let started = apply(compacting(), SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    let compacting_turn = apply(started, compaction_answered()).state;
    assert_eq!(compacting_turn.live_status(), Some(AgentStatus::Working));

    let sent = apply(compacting_turn, SessionEvent::SendTurn("after".to_string()));
    assert!(sent.effects.is_empty());

    let completed = apply(sent.state, turn_completed(TURN_ID, None));
    assert!(completed
        .effects
        .contains(&SessionEffect::Request(start_turn("after"))));
}

#[test]
fn a_turn_codex_starts_after_a_compaction_that_chose_another_model_keeps_the_applied_choice() {
    let other =
        |input: &str, revision| chosen_turn(input, Some("gpt-other"), Some("low"), revision);
    let compacting = apply(
        advance_to_waiting(),
        SessionEvent::SendChosenTurn(other("/compact", 1)),
    )
    .state;
    let answered = apply(compacting, compaction_answered()).state;
    let started = apply(answered, SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    let waiting = apply(started, turn_completed(TURN_ID, None)).state;
    let adopted = apply(
        waiting,
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    )
    .state;

    let sent = apply(adopted, SessionEvent::SendChosenTurn(other("after", 2)));
    assert!(sent.effects.is_empty());
    assert_eq!(sent.state.queued_turn_count(), 1);

    let completed = apply(sent.state, turn_completed(CODEX_TURN_ID, None));
    assert!(completed
        .effects
        .contains(&SessionEffect::Request(start_turn_with(
            "after",
            Some("gpt-other"),
            Some("low"),
        ))));
}

#[test]
fn a_message_choosing_the_applied_model_steers_into_a_turn_codex_started() {
    let adopted = apply(
        waiting_after_compaction(),
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    )
    .state;

    let sent = apply(
        adopted,
        SessionEvent::SendChosenTurn(chosen_turn(
            "more",
            Some(SELECTED_MODEL),
            Some(SELECTED_EFFORT),
            1,
        )),
    );
    assert!(matches!(
        sent.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::SteerTurn { turn_id, .. })] if turn_id == CODEX_TURN_ID
    ));
}

/// A follow-up sent into a turn Codex started after an unchanged compaction,
/// in a session whose model (when `model` is `None`) and effort are left to
/// Codex's defaults. Codex reports the default effort it chose as `medium`.
/// From the #445 round 2 review.
fn follow_up_into_a_codex_turn_on_defaults(model: Option<&str>) -> StateTransition {
    let state = CodexSessionState::new(
        PathBuf::from(WORKTREE_ROOT),
        model.map(str::to_string),
        None,
        None,
    );
    let initializing = apply(state, SessionEvent::Start).state;
    let opening = apply(initializing, initialize_response(&supported_user_agent())).state;
    let waiting = apply(
        opening,
        correlated(
            PendingOperation::StartThread {
                cwd: WORKTREE_ROOT.to_string(),
                model: model.map(str::to_string),
            },
            Ok(thread_opened(THREAD_ID, Some("medium"))),
        ),
    )
    .state;
    let compacting = apply(
        waiting,
        SessionEvent::SendChosenTurn(chosen_turn("/compact", model, None, 1)),
    )
    .state;
    let answered = apply(compacting, compaction_answered()).state;
    let started = apply(answered, SessionEvent::TurnStarted(TURN_ID.to_string())).state;
    let completed = apply(started, turn_completed(TURN_ID, None)).state;
    let adopted = apply(
        completed,
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    )
    .state;
    apply(
        adopted,
        SessionEvent::SendChosenTurn(chosen_turn("more", model, None, 2)),
    )
}

fn steered_into_the_codex_turn(sent: &StateTransition) -> bool {
    matches!(
        sent.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::SteerTurn { turn_id, .. })] if turn_id == CODEX_TURN_ID
    )
}

#[test]
fn a_follow_up_keeping_the_default_effort_steers_into_a_turn_codex_started() {
    let sent = follow_up_into_a_codex_turn_on_defaults(Some(SELECTED_MODEL));
    assert!(
        steered_into_the_codex_turn(&sent),
        "effects={:?}, queued={}",
        sent.effects,
        sent.state.queued_turn_count()
    );
}

#[test]
fn a_follow_up_keeping_the_default_model_and_effort_steers_into_a_turn_codex_started() {
    let sent = follow_up_into_a_codex_turn_on_defaults(None);
    assert!(
        steered_into_the_codex_turn(&sent),
        "effects={:?}, queued={}",
        sent.effects,
        sent.state.queued_turn_count()
    );
}

#[test]
fn a_turn_codex_starts_runs_on_the_choice_the_last_turn_start_applied() {
    let other =
        |input: &str, revision| chosen_turn(input, Some("gpt-other"), Some("low"), revision);
    let starting = apply(
        advance_to_waiting(),
        SessionEvent::SendChosenTurn(other("go", 1)),
    )
    .state;
    let working = apply(
        starting,
        correlated(
            start_turn_with("go", Some("gpt-other"), Some("low")),
            Ok(json!({"turn":{"id":EARLIER_TURN_ID}})),
        ),
    )
    .state;
    let waiting = apply(working, turn_completed(EARLIER_TURN_ID, None)).state;
    let adopted = apply(
        waiting,
        SessionEvent::TurnStarted(CODEX_TURN_ID.to_string()),
    )
    .state;

    let sent = apply(adopted, SessionEvent::SendChosenTurn(other("more", 2)));
    assert!(
        steered_into_the_codex_turn(&sent),
        "effects={:?}",
        sent.effects
    );
}
