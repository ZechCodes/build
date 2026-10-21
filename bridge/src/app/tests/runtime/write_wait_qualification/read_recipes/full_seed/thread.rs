//! Historical completed-failure read evidence; never a successful
//! provider delivery or runtime operation qualification claim. Native receipt
//! encoding is Delivered + execution_error (there is no Failed status).
use crate::app::AppState;
use crate::operation::{OperationReceipt, OperationStatus};
use crate::thread::{ArtifactKind, ThreadEventKind};
use serde_json::Value;

pub(super) const MESSAGE: &str = "Qualification immutable searchable conversation witness";
pub(super) const ACTIVITY: &str = "Qualification real reasoning activity witness";
pub(super) const REVISION: &str = "# Qualification retained artifact\n\nExact saved contents.\n";
const NOW: &str = "2026-10-21T00:00:00Z";

pub(super) struct ThreadSeed {
    pub thread_id: String,
    pub generation: u64,
    pub from: u64,
    pub through: u64,
    pub revision: String,
    pub operation: String,
    pub operation_value: Value,
    pub item_count_before: usize,
    pub item_count_after: usize,
}

pub(super) fn seed(app: &mut AppState, owner: &str, agent: &str) -> ThreadSeed {
    let mut active = app.take_run(owner).unwrap();
    let selected = active
        .agents
        .by_id_mut(agent)
        .expect("existing canonical read agent");
    assert_eq!(selected.conversation_id(), agent);
    let thread_id = selected.thread.id.clone();
    let generation = selected.thread.generation_revision;
    let before = selected.thread.total_item_count() as usize;
    let previous_sequence = selected.thread.last_sequence();
    selected.thread.post_user(MESSAGE, None, NOW);
    let posted_sequence = selected.thread.last_message_sequence();
    let operation = format!("qualification-historical-read-{agent}");
    selected
        .thread
        .bind_operation_messages(&operation, previous_sequence, posted_sequence);
    let from = selected.thread.push_event(
        ThreadEventKind::Reasoning,
        Some(ACTIVITY.into()),
        None,
        None,
        NOW,
    );
    let revision = selected
        .thread
        .add_revision(ArtifactKind::Plan, REVISION, NOW)
        .id;
    // Read all seed user messages as consumed, including this historical failed
    // operation's marker, so fixture start does not inherit delivery work.
    selected.thread.read_unread(NOW);
    let after = selected.thread.total_item_count() as usize;
    let receipt = OperationReceipt {
        operation_id: operation.clone(),
        method: crate::operation::THREAD_POST_METHOD.into(),
        entity_id: owner.into(),
        agent_id: agent.into(),
        conversation_id: agent.into(),
        choice_revision: selected.choice_revision,
        posted_sequence,
        message_start_sequence: posted_sequence,
        status: OperationStatus::Delivered,
        execution_error: Some("Qualification historical failed operation read witness".into()),
        request_hash: "qualification-historical-read-seed".into(),
        delivery: None,
        requested_by: None,
    };
    app.operation_ledger
        .stage_acceptance(owner.into(), receipt.clone());
    app.finish_run_mutation(owner.into(), active).unwrap();
    let saved = app
        .require_store()
        .unwrap()
        .operation(&operation)
        .unwrap()
        .unwrap();
    assert_eq!(saved.wire_value(), receipt.wire_value());
    ThreadSeed {
        thread_id,
        generation,
        from,
        through: from,
        revision,
        operation,
        operation_value: saved.wire_value(),
        item_count_before: before,
        item_count_after: after,
    }
}
