//! Actual queue policies must carry every required originating obligation.
use super::*;
use crate::timing::write_wait::{LogicalWrite, WriteContext, WriteSnapshot, WriteWaitRegistry};
use std::collections::BTreeSet;
use std::sync::Arc;

fn root(registry: &Arc<WriteWaitRegistry>, name: &str) -> LogicalWrite {
    let root = registry.begin(name, "thread.post", Instant::now());
    root.admitted();
    root
}

// Test enrollment only. Ordinary constructors initialize this opt-in field
// empty in the RED stage; real command capture is tested by post_reply.rs.
fn enrolled(root: &LogicalWrite, owner: &str, agent: &str, survives: bool) -> PendingAgentTurn {
    let mut turn = catch_up(owner, agent);
    turn.survives_refusal = survives;
    turn.logical_obligations.push(root.child());
    turn
}
fn contexts(turn: &PendingAgentTurn) -> Vec<WriteContext> {
    turn.logical_obligations
        .iter()
        .map(|child| child.context())
        .collect()
}
fn names(turns: &[PendingAgentTurn]) -> BTreeSet<String> {
    turns
        .iter()
        .flat_map(contexts)
        .flat_map(|context| context.original_ids())
        .collect()
}
fn complete(turns: &mut [PendingAgentTurn]) {
    // Unit fixture owns actual queue extraction; it settles the obligations
    // only after queue operations under test, never in response to root ACK.
    for turn in turns {
        for child in turn.logical_obligations.drain(..) {
            child.complete();
        }
    }
}
fn row(registry: &WriteWaitRegistry, name: &str) -> WriteSnapshot {
    registry
        .snapshot()
        .into_iter()
        .find(|row| row.original_id == name)
        .unwrap()
}
fn expected(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}
fn assert_clean(registry: &WriteWaitRegistry, originals: &[&str]) {
    for original in originals {
        let row = row(registry, original);
        assert!(row.root_finished && row.outstanding.is_empty(), "{row:?}");
        assert!(!row.cancelled && !row.abandoned, "{row:?}");
    }
}

#[test]
fn logical_write_delivery_queued_join_retains_each_original() {
    let registry = WriteWaitRegistry::new();
    let first = root(&registry, "queued-first");
    let second = root(&registry, "queued-second");
    let mut queue = DeliveryQueue::default();
    queue.enqueue(enrolled(&first, "owner", "agent", false));
    queue.enqueue_with_next_delivery(enrolled(&second, "owner", "agent", false));
    first.complete();
    second.complete();
    let mut ready = queue.take_ready(|_| false);
    let got = names(&ready);
    let one_delivery = ready.len();
    complete(&mut ready);
    assert_eq!(
        one_delivery, 1,
        "native catch-up coalescing stays unchanged"
    );
    assert_eq!(got, expected(&["queued-first", "queued-second"]));
    assert_clean(&registry, &["queued-first", "queued-second"]);
}

#[test]
fn logical_write_delivery_settling_join_retains_each_original() {
    let registry = WriteWaitRegistry::new();
    let first = root(&registry, "settle-first");
    let second = root(&registry, "settle-second");
    let mut queue = DeliveryQueue::default();
    queue.enqueue_after_settle_window(enrolled(&first, "owner", "agent", false));
    let first_deadline = queue.settle_wake_due().unwrap();
    queue.enqueue_after_settle_window(enrolled(&second, "owner", "agent", false));
    let no_later_wake = queue.settle_wake_due().is_none();
    first.complete();
    second.complete();
    queue.lapse_settle_windows();
    let mut ready = queue.take_ready(|_| false);
    let got = names(&ready);
    let one_delivery = ready.len();
    complete(&mut ready);
    queue.settle_wake_fired(first_deadline);
    assert!(
        no_later_wake,
        "a later request must not postpone the native deadline"
    );
    assert_eq!(one_delivery, 1);
    assert_eq!(got, expected(&["settle-first", "settle-second"]));
    assert_clean(&registry, &["settle-first", "settle-second"]);
}

#[test]
fn logical_write_delivery_ready_turn_absorbs_waiting_original() {
    let registry = WriteWaitRegistry::new();
    let notice = root(&registry, "waiting-notice");
    let direct = root(&registry, "ready-direct");
    let mut queue = DeliveryQueue::default();
    queue.enqueue_with_next_delivery(enrolled(&notice, "owner", "agent", false));
    queue.enqueue(enrolled(&direct, "owner", "agent", false));
    notice.complete();
    direct.complete();
    let mut ready = queue.take_ready(|_| false);
    let got = names(&ready);
    let one_delivery = ready.len();
    complete(&mut ready);
    assert_eq!(one_delivery, 1);
    assert_eq!(got, expected(&["waiting-notice", "ready-direct"]));
    assert_clean(&registry, &["waiting-notice", "ready-direct"]);
}

#[test]
fn logical_write_delivery_distinct_targets_retain_order_and_roots() {
    let registry = WriteWaitRegistry::new();
    let first = root(&registry, "target-one");
    let second = root(&registry, "target-two");
    let mut queue = DeliveryQueue::default();
    queue.enqueue(enrolled(&first, "owner", "agent-a", false));
    queue.enqueue(enrolled(&second, "owner", "agent-b", false));
    first.complete();
    second.complete();
    let mut ready = queue.take_ready(|_| false);
    let order: Vec<_> = ready.iter().map(|turn| turn.agent_id.clone()).collect();
    let got = names(&ready);
    let pending = [row(&registry, "target-one"), row(&registry, "target-two")];
    complete(&mut ready);
    assert_eq!(order, ["agent-a", "agent-b"]);
    assert_eq!(got, expected(&["target-one", "target-two"]));
    assert!(pending
        .iter()
        .all(|row| row.root_finished && row.outstanding.len() == 1));
    assert_clean(&registry, &["target-one", "target-two"]);
}

#[test]
fn logical_write_delivery_requeue_reset_hold_preserves_original_lifetime() {
    let registry = WriteWaitRegistry::new();
    let origin = root(&registry, "held-retry");
    let mut queue = DeliveryQueue::default();
    queue.enqueue(enrolled(&origin, "owner", "agent", false));
    origin.complete();
    let before = row(&registry, "held-retry");
    let mut ready = queue.take_ready(|_| false);
    let turn = ready.pop().unwrap();
    let ticket = queue.start(&turn);
    queue.requeue(turn);
    queue.settle(ticket);
    let held = queue.take_conversation("conversation-agent");
    let held_still_owed = row(&registry, "held-retry");
    queue.restore_conversation(held);
    let mut ready = queue.take_ready(|_| false);
    let got = names(&ready);
    let survives_reset_refusal = ready.iter().all(|turn| turn.survives_refusal);
    complete(&mut ready);
    assert_eq!(got, expected(&["held-retry"]));
    assert!(survives_reset_refusal);
    assert_eq!(held_still_owed.id, before.id);
    assert_eq!(held_still_owed.intake, before.intake);
    assert_eq!(held_still_owed.outstanding, before.outstanding);
    assert_clean(&registry, &["held-retry"]);
}

#[test]
fn logical_write_delivery_refusal_cancels_only_discarded_candidate() {
    let registry = WriteWaitRegistry::new();
    let earlier = root(&registry, "earlier-durable");
    let discarded = root(&registry, "discarded-candidate");
    let durable = root(&registry, "new-durable");
    let mut queue = DeliveryQueue::default();
    queue.enqueue(enrolled(&earlier, "earlier", "a", false));
    let checkpoint = queue.checkpoint();
    queue.enqueue(enrolled(&discarded, "discarded", "b", false));
    queue.enqueue(enrolled(&durable, "durable", "c", true));
    earlier.complete();
    discarded.complete();
    durable.complete();
    queue.refuse_since(checkpoint);
    let rejected = row(&registry, "discarded-candidate");
    let mut ready = queue.take_ready(|_| false);
    let order: Vec<_> = ready.iter().map(|turn| turn.owner.clone()).collect();
    let got = names(&ready);
    complete(&mut ready);
    assert_eq!(order, ["earlier", "durable"]);
    assert_eq!(got, expected(&["earlier-durable", "new-durable"]));
    assert!(rejected.root_finished && rejected.outstanding.is_empty());
    assert!(
        rejected.cancelled && !rejected.abandoned,
        "known candidate refusal is explicit: {rejected:?}"
    );
    assert_clean(&registry, &["earlier-durable", "new-durable"]);
}

#[test]
fn logical_write_delivery_coalescing_keeps_distinct_registry_epochs() {
    let old = WriteWaitRegistry::new();
    let new = WriteWaitRegistry::new();
    let first = root(&old, "old-epoch");
    let second = root(&new, "new-epoch");
    assert_eq!(
        first.id(),
        second.id(),
        "IDs alone cannot establish common collector identity"
    );
    let mut queue = DeliveryQueue::default();
    queue.enqueue(enrolled(&first, "owner", "agent", false));
    queue.enqueue_with_next_delivery(enrolled(&second, "owner", "agent", false));
    first.complete();
    second.complete();
    let mut ready = queue.take_ready(|_| false);
    let got = names(&ready);
    let captured: Vec<_> = ready.iter().flat_map(contexts).collect();
    let distinct_epochs = captured.len() == 2 && !captured[0].same_registry(&captured[1]);
    complete(&mut ready);
    assert_eq!(ready.len(), 1);
    assert_eq!(got, expected(&["old-epoch", "new-epoch"]));
    assert!(
        distinct_epochs,
        "numeric ID collisions must not merge registry epochs"
    );
    assert_clean(&old, &["old-epoch"]);
    assert_clean(&new, &["new-epoch"]);
}
