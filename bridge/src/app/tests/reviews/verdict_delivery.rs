//! The durable author outbox waits for provider receipt and never appends twice.
use super::*;
use crate::operation::OperationStatus;
use crate::store::ReviewVerdictDeliveryState;

fn author_review(state: &mut AppState, project: &str) -> (String, String, Value) {
    let (owner, author) = project_agent(state, project);
    let workspace_id = workspace(state, project, "durable verdict");
    let action = crate::mcp::BridgeAction::TrackerOpenReview {
        params: serde_json::from_value(json!({"workspace_id":workspace_id,
            "request_id":"durable-verdict","title":"Durable verdict","description":"Review"}))
        .unwrap(),
    };
    let review = state.agent_action(&owner, &author, action).unwrap();
    (owner, author, review)
}

fn verdict(state: &mut AppState, review: &Value, choice: &str) -> String {
    review_call(
        state,
        "tasks.comment",
        json!({"task_id":review["task"]["id"],
        "body":"The review reason", "opinion":{"snapshot_id":review["review"]["snapshots"][0]["id"],
        "verdict":choice}}),
    )["comment"]["id"]
        .as_str()
        .unwrap()
        .into()
}

fn receipt_messages(state: &AppState, owner: &str, agent: &str, operation: &str) -> usize {
    state
        .agent_conversation(owner, Some(agent))
        .unwrap()
        .items
        .iter()
        .filter(|item| {
            matches!(item, crate::thread::ThreadItem::Message(message)
            if message.operation_id.as_deref() == Some(operation))
        })
        .count()
}

#[test]
fn stopped_usage_limited_author_pending_survives_store_reopen_and_delivers_exactly_once() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let (owner, author, review) = author_review(&mut state, &project);
    let limited = crate::harness::SessionStatusSnapshot::new(crate::harness::AgentStatus::Waiting)
        .limited(crate::harness::usage_limit::UsageLimited {
            said: "usage limit".into(),
            resets_at: Some(time::OffsetDateTime::now_utc() + time::Duration::hours(1)),
        });
    state.record_usage_limit(&owner, &author, &limited, &mut Default::default());
    let comment = verdict(&mut state, &review, "approve");
    state.drain_review_verdict_deliveries().unwrap();
    let store = state.store.as_ref().unwrap();
    let delivery = store
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_eq!(delivery.state, ReviewVerdictDeliveryState::Pending);
    let receipt = store.operation(&delivery.operation_id).unwrap().unwrap();
    assert_eq!(receipt.status, OperationStatus::Queued);
    assert_eq!(
        receipt_messages(&state, &owner, &author, &delivery.operation_id),
        1
    );
    // No harness exists; durability is independent of a running author.
    state.store = Some(crate::store::Store::new(home.path().join("store")).unwrap());
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        receipt_messages(&state, &owner, &author, &delivery.operation_id),
        1
    );
    state
        .transition_delivery_operation(
            &delivery.operation_id,
            OperationStatus::Queued,
            OperationStatus::Delivered,
            None,
        )
        .unwrap();
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review_verdict_delivery(&comment)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Delivered
    );
    let retry = state.review_retry_verdict_delivery(
        crate::api::v1::reviews::ReviewRetryDeliveryParams {
            task_id: delivery.task_id.clone(),
            comment_id: comment,
        },
        crate::tracker::Actor::User,
    );
    assert!(retry.is_err(), "a completed handoff cannot be sent twice");
    assert_eq!(
        state
            .operation_receipt(&delivery.operation_id)
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Delivered
    );
    assert_eq!(
        receipt_messages(&state, &owner, &author, &delivery.operation_id),
        1
    );
}

#[test]
fn explicit_uncertain_retry_reuses_message_and_operation() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let (owner, author, review) = author_review(&mut state, &project);
    let comment = verdict(&mut state, &review, "request_changes");
    state.drain_review_verdict_deliveries().unwrap();
    let delivery = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    state
        .transition_delivery_operation(
            &delivery.operation_id,
            OperationStatus::Queued,
            OperationStatus::Uncertain,
            Some("provider receipt lost"),
        )
        .unwrap();
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review_verdict_delivery(&comment)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Failed
    );
    state
        .review_retry_verdict_delivery(
            crate::api::v1::reviews::ReviewRetryDeliveryParams {
                task_id: delivery.task_id,
                comment_id: comment.clone(),
            },
            crate::tracker::Actor::User,
        )
        .unwrap();
    let retried = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_eq!(retried.operation_id, delivery.operation_id);
    assert_eq!(retried.state, ReviewVerdictDeliveryState::Pending);
    assert_eq!(
        state
            .operation_receipt(&delivery.operation_id)
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Queued
    );
    assert_eq!(
        receipt_messages(&state, &owner, &author, &delivery.operation_id),
        1
    );
}

#[test]
fn legacy_author_falls_back_to_workspace_agent_and_missing_recipient_mentions_user_once() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "fallback");
    // Open without an agent; this is also the shape of legacy PR metadata.
    let review = review_call(
        &mut state,
        "tasks.review.open",
        json!({"workspace_id":workspace_id,
        "request_id":"legacy-no-author","title":"Fallback","description":"Review"}),
    );
    let owner = review_call(
        &mut state,
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace_id}),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent = review_call(&mut state, "agent.add", json!({"entity_id":owner}))["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let comment = verdict(&mut state, &review, "approve");
    state
        .store
        .as_ref()
        .unwrap()
        .bind_review_verdict_delivery(&comment, "removed-author")
        .unwrap();
    state.drain_review_verdict_deliveries().unwrap();
    let delivery = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_eq!(delivery.recipient_agent_id.as_deref(), Some(agent.as_str()));
    assert_eq!(
        receipt_messages(&state, &owner, &agent, &delivery.operation_id),
        1
    );

    let no_agent_workspace = workspace(&mut state, &project, "nobody");
    let no_author = review_call(
        &mut state,
        "tasks.review.open",
        json!({"workspace_id":no_agent_workspace,
        "request_id":"nobody","title":"Nobody","description":"Review"}),
    );
    let comment = verdict(&mut state, &no_author, "reject");
    state.drain_review_verdict_deliveries().unwrap();
    state.drain_review_verdict_deliveries().unwrap();
    let store = state.store.as_ref().unwrap();
    let failed = store
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, ReviewVerdictDeliveryState::Failed);
    let timeline = store.load_tracker_timeline(&failed.task_id).unwrap();
    let warnings = timeline
        .iter()
        .filter_map(|entry| match entry {
            crate::tracker::TimelineEntry::Comment(comment) if comment.mentions_user => {
                Some(comment)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].body.contains("Nobody received the verdict"));
    assert_eq!(warnings[0].author, crate::tracker::Actor::Build);
}

#[test]
fn failed_declined_operation_retains_payload_for_explicit_retry() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let (owner, author, review) = author_review(&mut state, &project);
    let comment = verdict(&mut state, &review, "approve");
    state.drain_review_verdict_deliveries().unwrap();
    let delivery = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    state
        .transition_delivery_operation(
            &delivery.operation_id,
            OperationStatus::Queued,
            OperationStatus::Delivered,
            Some("Author temporarily cannot receive"),
        )
        .unwrap();
    assert!(state
        .operation_receipt(&delivery.operation_id)
        .unwrap()
        .unwrap()
        .delivery
        .is_some());
    state.drain_review_verdict_deliveries().unwrap();
    state
        .review_retry_verdict_delivery(
            crate::api::v1::reviews::ReviewRetryDeliveryParams {
                task_id: delivery.task_id,
                comment_id: comment.clone(),
            },
            crate::tracker::Actor::User,
        )
        .unwrap();
    assert_eq!(
        state
            .operation_receipt(&delivery.operation_id)
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Queued
    );
    state
        .transition_delivery_operation(
            &delivery.operation_id,
            OperationStatus::Queued,
            OperationStatus::Delivered,
            None,
        )
        .unwrap();
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review_verdict_delivery(&comment)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Delivered
    );
    assert_eq!(
        receipt_messages(&state, &owner, &author, &delivery.operation_id),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn startup_service_delivers_missing_author_notice_without_client() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "no client");
    let review = review_call(
        &mut state,
        "tasks.review.open",
        json!({"workspace_id":workspace_id,
        "request_id":"no-client","title":"No client","description":"Review"}),
    );
    let comment = verdict(&mut state, &review, "reject");
    let store = state.store.as_ref().unwrap().clone();
    let shared = state.shared();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = store
                .load_review_verdict_delivery(&comment)
                .unwrap()
                .unwrap();
            if row.state == ReviewVerdictDeliveryState::Failed {
                let fallback = store
                    .load_tracker_comment(row.fallback_comment_id.as_deref().unwrap())
                    .unwrap()
                    .unwrap();
                assert!(fallback.mentions_user);
                assert!(fallback.body.contains("Nobody received"));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("startup service must drain without a client");
    drop(shared);
}

#[test]
fn queued_authors_cannot_starve_later_verdict_delivery_pages() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let (_, _, review) = author_review(&mut state, &project);
    for _ in 0..21 {
        verdict(&mut state, &review, "approve");
    }
    let no_agent = workspace(&mut state, &project, "last delivery");
    let no_author = review_call(
        &mut state,
        "tasks.review.open",
        json!({"workspace_id":no_agent,
        "request_id":"last-verdict","title":"Last","description":"Review"}),
    );
    let last = verdict(&mut state, &no_author, "reject");
    // First fills its page with queued author operations; subsequent polls
    // rotate them so a later recipient is still considered.
    for _ in 0..3 {
        state.drain_review_verdict_deliveries().unwrap();
    }
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review_verdict_delivery(&last)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Failed
    );
}

#[test]
fn removed_author_after_queue_safely_redirects_unconsumed_verdict_to_workspace_agent() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "author removed after queue");
    let owner = review_call(
        &mut state,
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace_id}),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let author = review_call(
        &mut state,
        "agent.add",
        json!({"entity_id":owner,"name":"Author"}),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let fallback = review_call(
        &mut state,
        "agent.add",
        json!({"entity_id":owner,"name":"Replacement"}),
    )["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let review = state
        .agent_action(
            &owner,
            &author,
            crate::mcp::BridgeAction::TrackerOpenReview {
                params: serde_json::from_value(
                    json!({"workspace_id":workspace_id,"request_id":"removed-after-queue",
            "title":"Removed author","description":"Review"}),
                )
                .unwrap(),
            },
        )
        .unwrap();
    let comment = verdict(&mut state, &review, "approve");
    state.drain_review_verdict_deliveries().unwrap();
    let initial = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .operation_receipt(&initial.operation_id)
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Queued
    );
    review_call(
        &mut state,
        "agent.remove",
        json!({"entity_id":owner,"agent_id":author}),
    );
    state.drain_review_verdict_deliveries().unwrap();
    let redirected = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(&comment)
        .unwrap()
        .unwrap();
    assert_ne!(redirected.operation_id, initial.operation_id);
    assert_eq!(
        redirected.recipient_agent_id.as_deref(),
        Some(fallback.as_str())
    );
    assert_eq!(
        state
            .operation_receipt(&initial.operation_id)
            .unwrap()
            .unwrap()
            .status,
        OperationStatus::Delivered
    );
    assert_eq!(
        receipt_messages(&state, &owner, &fallback, &redirected.operation_id),
        1
    );
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        receipt_messages(&state, &owner, &fallback, &redirected.operation_id),
        1
    );
}

#[test]
fn two_pending_verdicts_for_stopped_author_arrive_in_order_without_coalescing() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(home.path());
    let (owner, author, review) = author_review(&mut state, &project);
    let first = verdict(&mut state, &review, "request_changes");
    let second = verdict(&mut state, &review, "approve");
    let store = state.store.as_ref().unwrap().clone();
    let first = store.load_review_verdict_delivery(&first).unwrap().unwrap();
    let second = store
        .load_review_verdict_delivery(&second)
        .unwrap()
        .unwrap();
    assert_eq!(first.state, ReviewVerdictDeliveryState::Pending);
    assert_eq!(second.state, ReviewVerdictDeliveryState::Pending);
    state.drain_review_verdict_deliveries().unwrap();
    let received = state
        .agent_conversation(&owner, Some(&author))
        .unwrap()
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message) => message.operation_id.clone(),
            _ => None,
        })
        .filter(|operation| operation == &first.operation_id || operation == &second.operation_id)
        .collect::<Vec<_>>();
    assert_eq!(
        received,
        vec![first.operation_id.clone(), second.operation_id.clone()]
    );
    let mut pending = state.take_pending_turns();
    let mut handoffs = Vec::new();
    while let Some((turn, mark)) = pending.next_turn() {
        if let Some(operation) = turn.operation_id {
            if operation == first.operation_id || operation == second.operation_id {
                handoffs.push(operation.clone());
                state
                    .transition_delivery_operation(
                        &operation,
                        OperationStatus::Queued,
                        OperationStatus::Delivered,
                        None,
                    )
                    .unwrap();
            }
        }
        mark.settle(&mut state);
    }
    assert_eq!(
        handoffs,
        vec![first.operation_id.clone(), second.operation_id.clone()]
    );
    state.drain_review_verdict_deliveries().unwrap();
    assert_eq!(
        store
            .load_review_verdict_delivery(&first.comment_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Delivered
    );
    assert_eq!(
        store
            .load_review_verdict_delivery(&second.comment_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewVerdictDeliveryState::Delivered
    );
}
