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
                ask_to_name: false,
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
// - requeue-before-settle on Store claim error and DeliveryOutcome::Deferred

/// A turn that reads the agent's unread thread when it is sent, the way an
/// issue notice's does.
fn catch_up(owner: &str, agent: &str) -> PendingAgentTurn {
    PendingAgentTurn {
        wants_catch_up: true,
        ..turn(owner, agent, true, false)
    }
}

fn operation_turn(owner: &str, agent: &str) -> PendingAgentTurn {
    PendingAgentTurn {
        operation_id: Some(format!("op-{agent}")),
        ..turn(owner, agent, true, true)
    }
}

fn agents(turns: &[PendingAgentTurn]) -> Vec<&str> {
    turns.iter().map(|turn| turn.agent_id.as_str()).collect()
}

#[test]
fn notices_inside_the_settle_window_ride_one_turn() {
    let mut queue = DeliveryQueue::default();
    for _ in 0..3 {
        queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    }
    assert!(
        queue.take_ready(|_| false).is_empty(),
        "an idle tracker's notice waits the window out"
    );

    queue.lapse_settle_windows();
    let ready = queue.take_ready(|_| false);
    assert_eq!(agents(&ready), ["agent-a"], "three notices, one turn");
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn a_notice_after_its_turn_went_starts_a_new_one() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.lapse_settle_windows();
    assert_eq!(queue.take_ready(|_| false).len(), 1);

    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    assert!(queue.take_ready(|_| false).is_empty(), "a new window opens");
    queue.lapse_settle_windows();
    assert_eq!(queue.take_ready(|_| false).len(), 1);
}

#[test]
fn a_settling_turn_does_not_hold_up_another_agent() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.enqueue(catch_up("other", "agent-b"));
    assert_eq!(agents(&queue.take_ready(|_| false)), ["agent-b"]);
}

#[test]
fn an_operation_turn_goes_now_and_takes_the_settling_notice_with_it() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.enqueue(operation_turn("tracker", "agent-a"));

    let ready = queue.take_ready(|_| false);
    assert_eq!(ready.len(), 2, "neither waits the window");
    assert_eq!(ready[0].operation_id.as_deref(), Some("op-agent-a"));
    assert!(ready[1].operation_id.is_none());
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn a_turn_that_reads_the_thread_carries_the_settling_notice() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    let mut dispatch = catch_up("tracker", "agent-a");
    dispatch.phase = "dispatch";
    queue.enqueue(dispatch);

    let ready = queue.take_ready(|_| false);
    assert_eq!(
        ready.len(),
        1,
        "the dispatch reads the notice off the thread"
    );
    assert_eq!(ready[0].phase, "dispatch");
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn a_notice_joins_a_catch_up_turn_already_queued() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue(catch_up("tracker", "agent-a"));
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    assert_eq!(queue.take_ready(|_| false).len(), 1, "one turn reads both");
}

#[test]
fn a_rider_never_wakes_its_agent_alone() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_with_next_delivery(catch_up("worker", "agent-a"));
    queue.lapse_settle_windows();
    assert!(queue.take_ready(|_| false).is_empty());
    assert!(
        !queue.holds_agent(&catch_up("worker", "agent-a").tab_key()),
        "a rider is not a turn on its way"
    );
    assert!(!queue.holds_owner("worker"));

    queue.enqueue(catch_up("worker", "agent-a"));
    assert_eq!(
        queue.take_ready(|_| false).len(),
        1,
        "the next delivery reads the rider off the thread"
    );
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn a_notice_for_an_agent_with_a_rider_wakes_it_once_after_the_window() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_with_next_delivery(catch_up("worker", "agent-a"));
    queue.enqueue_after_settle_window(catch_up("worker", "agent-a"));
    assert!(queue.take_ready(|_| false).is_empty());
    queue.lapse_settle_windows();
    assert_eq!(queue.take_ready(|_| false).len(), 1);
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn an_in_flight_agent_keeps_its_settled_notice_until_it_is_free() {
    let mut queue = DeliveryQueue::default();
    let delivering = catch_up("tracker", "agent-a");
    let ticket = queue.start(&delivering);
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.lapse_settle_windows();
    assert!(queue.take_ready(|_| false).is_empty());
    assert_eq!(queue.settle_wake_due(), None, "no wake for a lapsed window");

    queue.settle(ticket);
    assert_eq!(queue.take_ready(|_| false).len(), 1);
}

#[test]
fn a_settle_window_asks_for_one_wake_at_its_end() {
    let mut queue = DeliveryQueue::default();
    assert_eq!(queue.settle_wake_due(), None);
    let before = std::time::Instant::now();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    let wake = queue.settle_wake_due().expect("the window's end is a wake");
    assert!(wake >= before + NOTICE_SETTLE_WINDOW);
    assert_eq!(queue.settle_wake_due(), None, "already asked for");

    queue.settle_wake_fired(wake);
    assert_eq!(queue.settle_wake_due(), Some(wake), "asked for again");
}

#[test]
fn a_refused_request_drops_the_notice_it_deferred() {
    let mut queue = DeliveryQueue::default();
    let checkpoint = queue.checkpoint();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.refuse_since(checkpoint);
    queue.lapse_settle_windows();
    assert!(queue.take_ready(|_| false).is_empty());
}

#[test]
fn removing_an_agent_removes_its_settling_turn() {
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(catch_up("tracker", "agent-a"));
    queue.retain_queued(|turn| turn.agent_id != "agent-a");
    queue.lapse_settle_windows();
    assert!(queue.take_ready(|_| false).is_empty());
}
