use super::*;
use crate::app::TurnText;
use crate::operation::{DeliveryIntent, OperationPayload, OperationReceipt, OperationStatus};

fn turn(owner: &str, agent: &str, says: bool, survives_refusal: bool) -> PendingAgentTurn {
    PendingAgentTurn {
        operation_id: None,
        root: std::path::PathBuf::from("/tmp/stage8-queue"),
        owner: owner.into(),
        agent_id: agent.into(),
        conversation_id: format!("conversation-{agent}"),
        model_choice: crate::models::ModelChoice::default(),
        choice_revision: 0,
        interrupt: false,
        say: says.then(|| TurnText {
            cold: "cold".into(),
            warm: "warm".into(),
        }),
        phase: "test",
        wants_catch_up: false,
        survives_refusal,
    }
}

#[test]
fn refusal_keeps_earlier_and_surviving_turns_in_order() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(turn("earlier", "a", true, false));
    let checkpoint = queue.checkpoint();
    queue.enqueue(turn("discard", "b", true, false));
    queue.enqueue(turn("durable", "c", true, true));
    queue.refuse_since(checkpoint);
    assert_eq!(
        queue
            .queued()
            .map(|turn| turn.owner.as_str())
            .collect::<Vec<_>>(),
        ["earlier", "durable"]
    );
}

#[test]
fn refusal_checkpoint_is_clamped_after_an_unrelated_removal() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(turn("first", "a", true, false));
    queue.enqueue(turn("removed", "b", true, false));
    let checkpoint = queue.checkpoint();
    queue.retain_queued(|turn| turn.owner != "removed");
    queue.refuse_since(checkpoint);
    assert_eq!(queue.queued().next().unwrap().owner, "first");
}

#[test]
fn tickets_count_each_owner_and_only_agents_that_are_told() {
    let mut queue = DeliveryQueue::default();
    let speaking = turn("owner", "speaking", true, false);
    let silent = turn("owner", "silent", false, false);
    let speaking_key = speaking.tab_key();
    let silent_key = silent.tab_key();
    let speaking_ticket = queue.start(&speaking);
    let silent_ticket = queue.start(&silent);
    assert!(queue.holds_owner("owner"));
    assert!(queue.holds_agent(&speaking_key));
    assert!(!queue.holds_agent(&silent_key));
    queue.settle(speaking_ticket);
    assert!(queue.holds_owner("owner"));
    assert!(!queue.holds_agent(&speaking_key));
    queue.settle(silent_ticket);
    assert!(!queue.holds_owner("owner"));
}

#[test]
fn held_and_ready_turns_each_keep_their_order() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(turn("held-1", "a", true, false));
    queue.enqueue(turn("ready-1", "b", true, false));
    queue.enqueue(turn("held-2", "c", true, false));
    queue.enqueue(turn("ready-2", "d", true, false));
    let ready = queue.take_ready(|turn| turn.owner.starts_with("held"));
    assert_eq!(
        ready
            .iter()
            .map(|turn| turn.owner.as_str())
            .collect::<Vec<_>>(),
        ["ready-1", "ready-2"]
    );
    assert_eq!(
        queue
            .queued()
            .map(|turn| turn.owner.as_str())
            .collect::<Vec<_>>(),
        ["held-1", "held-2"]
    );
}

fn delivery_receipt(operation_id: &str) -> OperationReceipt {
    OperationReceipt {
        operation_id: operation_id.into(),
        method: crate::operation::THREAD_POST_METHOD.into(),
        entity_id: "owner".into(),
        agent_id: "agent".into(),
        conversation_id: "conversation-agent".into(),
        choice_revision: 0,
        posted_sequence: 1,
        message_start_sequence: 1,
        status: OperationStatus::Queued,
        execution_error: None,
        request_hash: "hash".into(),
        delivery: Some(DeliveryIntent {
            root: "/tmp/stage8-queue".into(),
            owner_id: "owner".into(),
            agent_id: "agent".into(),
            model_choice: crate::models::ModelChoice::default(),
            choice_revision: 0,
            interrupt: false,
            payload: Some(OperationPayload {
                start_sequence: 1,
                end_sequence: 1,
                messages: Vec::new(),
                prior_context: String::new(),
            }),
        }),
        requested_by: None,
    }
}

#[test]
fn attaching_an_operation_rebuilds_one_scoped_cold_protocol() {
    let mut queue = DeliveryQueue::default();
    let mut pending = turn("owner", "agent", true, false);
    pending.say = Some(TurnText {
        cold: crate::orchestrator::conversation_prompt("do planned work"),
        warm: "do planned work".into(),
    });
    queue.enqueue(pending);

    queue
        .attach_plan_operation(&delivery_receipt("operation-7"))
        .unwrap();

    let said = queue.queued_last().unwrap().said();
    assert_eq!(said.cold.matches("Build conversation protocol:").count(), 1);
    assert!(said.cold.contains("do planned work"), "{}", said.cold);
    assert!(
        said.cold.contains("reviewer operation `operation-7`"),
        "{}",
        said.cold
    );
    assert!(!said.cold.contains("process every unread Issue message"));
    assert_eq!(said.warm.matches("Build conversation protocol:").count(), 0);
}

#[test]
fn recovered_operation_keeps_cold_start_protocol_and_a_warm_exact_packet() {
    let receipt = delivery_receipt("operation-recovered");
    let turn = PendingAgentTurn::for_delivery_operation(&receipt).unwrap();
    let said = turn.said();
    assert!(said.cold.contains("`set_topic`"), "{}", said.cold);
    assert!(
        said.cold
            .contains("reviewer operation `operation-recovered`"),
        "{}",
        said.cold
    );
    assert!(!said.cold.contains("process every unread Issue message"));
    assert!(!said.warm.contains("Build conversation protocol:"));
    assert!(said.warm.contains("Process only reviewer operation"));
}

#[test]
fn an_in_flight_agent_holds_its_followup_while_another_agent_proceeds() {
    let mut queue = DeliveryQueue::default();
    let delivering = turn("owner-a", "agent-a", true, false);
    let ticket = queue.start(&delivering);
    queue.enqueue(turn("owner-a", "agent-a", true, false));
    queue.enqueue(turn("owner-b", "agent-b", true, false));

    let ready = queue.take_ready(|_| false);
    assert_eq!(
        ready
            .iter()
            .map(|turn| turn.agent_id.as_str())
            .collect::<Vec<_>>(),
        ["agent-b"]
    );
    assert_eq!(queue.queued().next().unwrap().agent_id, "agent-a");

    queue.settle(ticket);
    let released = queue.take_ready(|_| false);
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].agent_id, "agent-a");
    assert!(queue.queued_is_empty());
}

// Runtime integration retains these existing tests because a callback-free core
// cannot itself prove SettlingHandle behavior:
// - a_delivery_that_panics_gives_its_in_flight_marks_back
// - disabling_triage_stops_a_pass_already_drained_for_delivery
// - requeue-before-settle on Store claim error and DeliveryOutcome::Deferred
