use super::*;
use crate::app::TurnText;

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

// Runtime integration retains these existing tests because a callback-free core
// cannot itself prove SettlingHandle behavior:
// - a_delivery_that_panics_gives_its_in_flight_marks_back
// - disabling_triage_stops_a_pass_already_drained_for_delivery
// - requeue-before-settle on Store claim error and DeliveryOutcome::Deferred
