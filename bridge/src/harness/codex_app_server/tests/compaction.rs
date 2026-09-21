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
