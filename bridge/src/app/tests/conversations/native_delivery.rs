use super::*;
use crate::operation::OperationPayload;
use crate::thread::{MessageDeliveryStatus, ThreadItem, ThreadMessage};

fn operation_message<'a>(
    state: &'a AppState,
    owner: &str,
    agent: &str,
    operation: &str,
) -> &'a ThreadMessage {
    state
        .agent_conversation(owner, Some(agent))
        .unwrap()
        .items
        .iter()
        .find_map(|item| match item {
            ThreadItem::Message(message) if message.operation_id.as_deref() == Some(operation) => {
                Some(message)
            }
            _ => None,
        })
        .expect("the operation owns a message")
}

fn post_operation(state: &mut AppState, owner: &str, agent: &str, operation: &str, body: &str) {
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": owner,
            "agent_id": agent,
            "conversation_id": agent,
            "operation_id": operation,
            "body": body,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
}

fn message_by_body<'a>(
    state: &'a AppState,
    owner: &str,
    agent: &str,
    body: &str,
) -> &'a ThreadMessage {
    state
        .agent_conversation(owner, Some(agent))
        .unwrap()
        .items
        .iter()
        .find_map(|item| match item {
            ThreadItem::Message(message) if message.body == body => Some(message),
            _ => None,
        })
        .expect("the posted message is present")
}

#[test]
fn native_receipt_sees_only_its_exact_queued_operation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "native-operation-receipt");
    let agent_id = primary_agent_id(&state, &run_id);

    post_operation(&mut state, &run_id, &agent_id, "operation-first", "first");
    post_operation(&mut state, &run_id, &agent_id, "operation-second", "second");
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "operation-first").delivery_status,
        Some(MessageDeliveryStatus::Queued)
    );
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "operation-second").delivery_status,
        Some(MessageDeliveryStatus::Queued)
    );

    state
        .record_operation_delivery_status("operation-first", MessageDeliveryStatus::Sent)
        .unwrap();
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "operation-first").delivery_status,
        Some(MessageDeliveryStatus::Sent),
        "provider handoff alone must not claim that the model saw the turn"
    );

    state
        .record_native_operation_seen(&run_id, &agent_id, "operation-first")
        .unwrap();
    let first = operation_message(&state, &run_id, &agent_id, "operation-first");
    assert_eq!(first.delivery_status, Some(MessageDeliveryStatus::Seen));
    assert!(first.seen_at.is_some());
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "operation-second").delivery_status,
        Some(MessageDeliveryStatus::Queued),
        "one native receipt cannot acknowledge another queued turn"
    );

    let updated_sequence = first.updated_sequence;
    state
        .record_native_operation_seen(&run_id, &agent_id, "operation-first")
        .unwrap();
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "operation-first").updated_sequence,
        updated_sequence,
        "repeated receipt observation must be idempotent"
    );
}

#[test]
fn native_receipt_requires_the_operation_owner_and_agent_binding() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "native-receipt-binding");
    let agent_id = primary_agent_id(&state, &run_id);
    post_operation(
        &mut state,
        &run_id,
        &agent_id,
        "bound-operation",
        "private turn",
    );

    let other_agent = state
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .add(&run_id, ModelChoice::default(), &now_rfc3339())
        .id
        .clone();
    assert!(state
        .record_native_operation_seen(&run_id, &other_agent, "bound-operation")
        .unwrap_err()
        .contains("does not belong"));
    assert!(state
        .record_native_operation_seen("wrong-owner", &agent_id, "bound-operation")
        .is_err());
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "bound-operation").delivery_status,
        Some(MessageDeliveryStatus::Queued)
    );
}

#[test]
fn seen_delivery_is_terminal_and_survives_reload() {
    let (dir, repo) = init_repo();
    let run_id;
    let agent_id;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "native-seen-terminal");
        agent_id = primary_agent_id(&state, &run_id);
        post_operation(
            &mut state,
            &run_id,
            &agent_id,
            "terminal-operation",
            "remember me",
        );
        state
            .record_native_operation_seen(&run_id, &agent_id, "terminal-operation")
            .unwrap();
        state
            .record_operation_delivery_status("terminal-operation", MessageDeliveryStatus::Sent)
            .unwrap();
        assert_eq!(
            operation_message(&state, &run_id, &agent_id, "terminal-operation").delivery_status,
            Some(MessageDeliveryStatus::Seen),
            "a late sent callback cannot downgrade an observed turn"
        );
    }

    let reloaded = qa_state(&repo, dir.path());
    let message = operation_message(&reloaded, &run_id, &agent_id, "terminal-operation");
    assert_eq!(message.body, "remember me");
    assert_eq!(message.delivery_status, Some(MessageDeliveryStatus::Seen));
    assert!(message.seen_at.is_some());
}

#[test]
fn legacy_native_receipt_marks_only_operationless_messages_seen() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "legacy-native-receipt");
    let agent_id = primary_agent_id(&state, &run_id);

    let legacy_post = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "body": "legacy turn",
        }),
    ));
    assert_eq!(legacy_post["ok"], true, "{legacy_post:?}");
    post_operation(
        &mut state,
        &run_id,
        &agent_id,
        "real-operation",
        "managed turn",
    );
    let legacy_sequence = message_by_body(&state, &run_id, &agent_id, "legacy turn").sequence;
    let managed_sequence = operation_message(&state, &run_id, &agent_id, "real-operation").sequence;
    let token = format!("@legacy/{legacy_sequence},{managed_sequence}");

    assert!(state
        .record_legacy_native_receipt(&run_id, &agent_id, &token, MessageDeliveryStatus::Seen,)
        .unwrap());
    let legacy = message_by_body(&state, &run_id, &agent_id, "legacy turn");
    assert_eq!(legacy.delivery_status, Some(MessageDeliveryStatus::Seen));
    assert!(legacy.seen_at.is_some());
    let managed = operation_message(&state, &run_id, &agent_id, "real-operation");
    assert_eq!(managed.delivery_status, Some(MessageDeliveryStatus::Queued));
    assert!(managed.seen_at.is_none());
}

#[test]
fn legacy_native_receipt_parser_rejects_false_or_invalid_correlations() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "legacy-receipt-parser");
    let agent_id = primary_agent_id(&state, &run_id);
    post_operation(
        &mut state,
        &run_id,
        &agent_id,
        "real-operation",
        "managed turn",
    );

    assert!(!state
        .record_legacy_native_receipt(
            &run_id,
            &agent_id,
            "real-operation",
            MessageDeliveryStatus::Seen,
        )
        .unwrap());
    for token in [
        "@legacy/",
        "@legacy/0",
        "@legacy/2,1",
        "@legacy/1,1",
        "@legacy/1/2",
    ] {
        assert!(
            state
                .record_legacy_native_receipt(
                    &run_id,
                    &agent_id,
                    token,
                    MessageDeliveryStatus::Seen,
                )
                .is_err(),
            "{token} must not correlate to messages"
        );
    }
    assert_eq!(
        operation_message(&state, &run_id, &agent_id, "real-operation").delivery_status,
        Some(MessageDeliveryStatus::Queued),
        "unrecognized receipts cannot become native MCP reads"
    );
}

#[test]
fn legacy_native_receipt_preserves_unlisted_messages_between_its_sequences() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "legacy-receipt-gaps");
    let agent_id = primary_agent_id(&state, &run_id);
    for body in ["queued first", "separate middle", "queued last"] {
        let posted = state.handle(req(
            "thread.post",
            json!({
                "entity_id": run_id,
                "agent_id": agent_id,
                "conversation_id": agent_id,
                "body": body,
            }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
    }
    let first = message_by_body(&state, &run_id, &agent_id, "queued first").clone();
    let middle = message_by_body(&state, &run_id, &agent_id, "separate middle").clone();
    let last = message_by_body(&state, &run_id, &agent_id, "queued last").clone();
    state
        .record_legacy_delivery_status(
            &run_id,
            &agent_id,
            &OperationPayload {
                start_sequence: middle.sequence,
                end_sequence: middle.sequence,
                messages: vec![middle.clone()],
                prior_context: String::new(),
            },
            MessageDeliveryStatus::Uncertain,
        )
        .unwrap();

    let token = format!("@legacy/{},{}", first.sequence, last.sequence);
    state
        .record_legacy_native_receipt(&run_id, &agent_id, &token, MessageDeliveryStatus::Seen)
        .unwrap();
    assert_eq!(
        message_by_body(&state, &run_id, &agent_id, "queued first").delivery_status,
        Some(MessageDeliveryStatus::Seen)
    );
    assert_eq!(
        message_by_body(&state, &run_id, &agent_id, "separate middle").delivery_status,
        Some(MessageDeliveryStatus::Uncertain),
        "a receipt must not fill numeric gaps with messages absent from its prompt"
    );
    assert_eq!(
        message_by_body(&state, &run_id, &agent_id, "queued last").delivery_status,
        Some(MessageDeliveryStatus::Seen)
    );
}

#[tokio::test]
async fn buffered_native_receipt_survives_the_session_object_being_dropped() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "buffered-native-receipt");
    let agent_id = primary_agent_id(&app, &run_id);
    post_operation(
        &mut app,
        &run_id,
        &agent_id,
        "buffered-operation",
        "outlives its session",
    );
    let conversation_id = app
        .resolve_conversation_address(&run_id, Some(&agent_id))
        .unwrap()
        .conversation_id;
    let instance = crate::thread::SessionInstance {
        id: "removed-session".into(),
        entity_id: run_id.clone(),
        agent_id: agent_id.clone(),
        conversation_id,
        checkout: dir.path().display().to_string(),
    };
    let (_receipt_tx, receipt_rx) =
        tokio::sync::watch::channel(crate::harness::TurnReceiptSnapshot {
            seen_operation_ids: vec!["buffered-operation".into()],
            uncertain_operation_ids: Vec::new(),
        });
    let session: Arc<dyn AgentSession> =
        Arc::new(DictatedSession::reporting(AgentStatus::Working).watching_receipts(receipt_rx));
    let state = app.shared();

    crate::app::runtime::delivery::receipts::spawn_receipt_pump(&state, &session, Some(instance));
    drop(session);

    wait_for(Duration::from_secs(2), || {
        let app = state.lock().unwrap();
        (operation_message(&app, &run_id, &agent_id, "buffered-operation").delivery_status
            == Some(MessageDeliveryStatus::Seen))
        .then_some(())
    })
    .await
    .expect("the pump consumed the receipt without a registry or live session object");
}

#[tokio::test]
async fn captured_conversation_mismatch_rejects_a_buffered_legacy_receipt() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut app, &repo, dir.path(), "stale-legacy-receipt");
    let agent_id = primary_agent_id(&app, &run_id);
    let posted = app.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": agent_id,
            "body": "belongs to old conversation",
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let old_conversation = app
        .resolve_conversation_address(&run_id, Some(&agent_id))
        .unwrap()
        .conversation_id;
    let sequence =
        message_by_body(&app, &run_id, &agent_id, "belongs to old conversation").sequence;
    let replacement = app
        .runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .add(&run_id, ModelChoice::default(), &now_rfc3339())
        .id
        .clone();
    app.runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .by_id_mut(&agent_id)
        .unwrap()
        .bind_conversation(&replacement);
    let instance = crate::thread::SessionInstance {
        id: "stale-session".into(),
        entity_id: run_id.clone(),
        agent_id: agent_id.clone(),
        conversation_id: old_conversation.clone(),
        checkout: dir.path().display().to_string(),
    };
    let (_receipt_tx, receipt_rx) =
        tokio::sync::watch::channel(crate::harness::TurnReceiptSnapshot {
            seen_operation_ids: vec![format!("@legacy/{sequence}")],
            uncertain_operation_ids: Vec::new(),
        });
    let session: Arc<dyn AgentSession> =
        Arc::new(DictatedSession::reporting(AgentStatus::Working).watching_receipts(receipt_rx));
    let state = app.shared();

    crate::app::runtime::delivery::receipts::spawn_receipt_pump(&state, &session, Some(instance));
    drop(session);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut app = state.lock().unwrap();
    app.runs
        .get_mut(&run_id)
        .unwrap()
        .agents
        .by_id_mut(&agent_id)
        .unwrap()
        .bind_conversation(old_conversation);
    let old_message = message_by_body(&app, &run_id, &agent_id, "belongs to old conversation");
    assert!(old_message.seen_at.is_none());
    assert_ne!(
        old_message.delivery_status,
        Some(MessageDeliveryStatus::Seen)
    );
}

#[test]
fn restart_hydrates_delivery_recovery_before_restoring_threads() {
    let (dir, repo) = init_repo();
    let run_id;
    let agent_id;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = adopted_run(&mut state, &repo, dir.path(), "native-recovery-order");
        agent_id = primary_agent_id(&state, &run_id);
        post_operation(
            &mut state,
            &run_id,
            &agent_id,
            "delivered-before-restart",
            "do not replay me",
        );
        assert!(state
            .store
            .as_ref()
            .unwrap()
            .transition_operation(
                "delivered-before-restart",
                crate::operation::OperationStatus::Queued,
                crate::operation::OperationStatus::Delivered,
                None,
            )
            .unwrap());
    }

    let mut restarted = qa_state(&repo, dir.path());
    let recovered = operation_message(&restarted, &run_id, &agent_id, "delivered-before-restart");
    assert_eq!(
        recovered.delivery_status,
        Some(MessageDeliveryStatus::Uncertain)
    );
    let recovered_sequence = recovered.updated_sequence;

    post_operation(
        &mut restarted,
        &run_id,
        &agent_id,
        "after-recovery",
        "new turn",
    );
    assert!(
        operation_message(&restarted, &run_id, &agent_id, "after-recovery").sequence
            > recovered_sequence,
        "restoration must retain the sequence minted by recovery"
    );
}
