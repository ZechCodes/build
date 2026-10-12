use super::*;
use crate::store::DeliveryIntent;
use crate::tracker::{Actor, Assignee, Task, TaskEvent, TaskEventKind};
use serde_json::json;

const PROJECT: &str = "/durable-delivery";
const NOW: &str = "2026-10-12T20:00:00Z";

fn assignment(store: &Store) -> (Task, DeliveryIntent) {
    let mut task = store
        .create_tracker_task(Task::drafted(PROJECT, "brief", Actor::User, NOW), &[])
        .unwrap();
    task.assignee = Some(Assignee::Agent {
        agent_id: "agent-1".into(),
    });
    let intent = DeliveryIntent {
        id: String::new(),
        task_id: task.id.clone(),
        project_path: PROJECT.into(),
        generation: 0,
        target: json!({"kind":"agent","agent_id":"agent-1"}),
        actor: Actor::User,
        sender: None,
        note: Some("original note".into()),
        operation_id: String::new(),
        state: "queued".into(),
        reservation: serde_json::Value::Null,
        admitted: None,
        error: None,
    };
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Assigned,
        json!({}),
        NOW,
    );
    let intent = store
        .accept_delivery_intent(&task, &[event], intent)
        .unwrap();
    (task, intent)
}

#[test]
fn deferred_dispatch_retry_freezes_message() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (mut task, intent) = assignment(&store);
    assert!(store
        .claim_delivery_intent(&intent.id, intent.generation)
        .unwrap()
        .is_some());
    let reservation = json!({"workspace_id":"workspace-reserved","agent_id":"agent-1"});
    let admitted = json!({"body":"original brief","thread_id":"thread-1","generation":4,"closed_without_done":["task-closed"]});
    store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            reservation.clone(),
            Some(admitted.clone()),
        )
        .unwrap();
    store
        .fail_delivery_intent(&intent.id, intent.generation, "temporary refusal")
        .unwrap();
    task.title = "renamed after admission".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let retry = store.retry_delivery_intent(&intent.id).unwrap();
    assert_eq!(retry.operation_id, intent.operation_id);
    assert_eq!(retry.reservation, reservation);
    assert_eq!(retry.admitted, Some(admitted.clone()));
    assert!(store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            json!({"workspace_id":"replacement"}),
            Some(admitted.clone())
        )
        .is_err());
    assert!(store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            reservation.clone(),
            Some(json!({"body":"changed"}))
        )
        .is_err());
    store
        .checkpoint_delivery_intent(&intent.id, intent.generation, reservation, Some(admitted))
        .unwrap();
}

#[test]
fn durable_delivery_generation_and_live_graph_fence_saved_work() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (task, first) = assignment(&store);
    let blocker = store
        .create_tracker_task(Task::drafted(PROJECT, "blocker", Actor::User, NOW), &[])
        .unwrap();
    store
        .edit_waiting_on(
            PROJECT,
            &task.id,
            &[blocker.id.clone()],
            &[],
            &Actor::User,
            NOW,
        )
        .unwrap();
    assert!(!store
        .delivery_intent_is_current(&first.id, first.generation)
        .unwrap());
    let second = store
        .accept_delivery_intent(
            &task,
            &[],
            DeliveryIntent {
                id: String::new(),
                operation_id: String::new(),
                ..first.clone()
            },
        )
        .unwrap();
    assert!(second.generation > first.generation);
    assert!(!store
        .delivery_authority_current(&first.id, first.generation)
        .unwrap());
    assert_eq!(second.state, "waiting");
    store
        .edit_waiting_on(PROJECT, &task.id, &[], &[blocker.id], &Actor::User, NOW)
        .unwrap();
    assert_eq!(store.delivery_intents_ready(1).unwrap().len(), 1);
    assert!(store
        .delivery_intent_is_current(&second.id, second.generation)
        .unwrap());
    let mut unassigned = task.clone();
    unassigned.assignee = None;
    store
        .save_tracker_task_activity(&unassigned, &[], &[])
        .unwrap();
    assert!(!store
        .delivery_authority_current(&second.id, second.generation)
        .unwrap());
    assert!(store.delivery_intents_ready(1).unwrap().is_empty());
}

#[test]
fn delivery_receipt_recovery_keeps_reblocked_task_column_and_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (mut task, intent) = assignment(&store);
    store
        .claim_delivery_intent(&intent.id, intent.generation)
        .unwrap();
    store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            json!({}),
            Some(json!({"body":"frozen"})),
        )
        .unwrap();
    let mut plan = super::support::plan_record("owner-delivery");
    plan.agents[0].thread.post_user("frozen", None, NOW);
    let mut receipt = super::support::queued_operation(&intent.operation_id, 1);
    receipt.entity_id = "owner-delivery".into();
    store
        .accept_thread_post("owner-delivery", &plan.agents, &receipt)
        .unwrap();
    let blocker = store
        .create_tracker_task(
            Task::drafted(PROJECT, "reopened blocker", Actor::User, NOW),
            &[],
        )
        .unwrap();
    store
        .edit_waiting_on(PROJECT, &task.id, &[blocker.id], &[], &Actor::User, NOW)
        .unwrap();
    assert!(!store
        .delivery_intent_is_current(&intent.id, intent.generation)
        .unwrap());
    assert_eq!(store.delivery_intents_ready(1).unwrap().len(), 1);
    let before = task.status.clone();
    task.status = "in_progress".into();
    let moved = TaskEvent::new(
        &task.id,
        Actor::Build,
        TaskEventKind::Moved,
        json!({"from":before,"to":"in_progress"}),
        NOW,
    );
    let dispatched = TaskEvent::new(
        &task.id,
        Actor::Build,
        TaskEventKind::Dispatched,
        json!({"operation_id":intent.operation_id}),
        NOW,
    );
    assert!(store
        .settle_delivery_intent(&intent.id, intent.generation, &task, &[moved, dispatched])
        .unwrap());
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().status,
        before
    );
    let timeline = store.load_tracker_timeline(&task.id).unwrap();
    assert!(!timeline.iter().any(|entry| matches!(entry, crate::tracker::TimelineEntry::Event(event) if event.kind == TaskEventKind::Moved && event.payload["to"] == "in_progress")), "receipt recovery cannot record a column move it refused");
    assert!(store.delivery_intents_ready(1).unwrap().is_empty());
}

#[test]
fn delivery_admission_fences_graph_writes_and_failed_retry() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (task, intent) = assignment(&store);
    store
        .claim_delivery_intent(&intent.id, intent.generation)
        .unwrap();
    store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            json!({}),
            Some(json!({"body":"saved"})),
        )
        .unwrap();
    let blocker = store
        .create_tracker_task(Task::drafted(PROJECT, "new blocker", Actor::User, NOW), &[])
        .unwrap();
    store
        .edit_waiting_on(
            PROJECT,
            &task.id,
            &[blocker.id.clone()],
            &[],
            &Actor::User,
            NOW,
        )
        .unwrap();
    let mut plan = super::support::plan_record("owner-fenced");
    plan.agents[0].thread.post_user("saved", None, NOW);
    let receipt = super::support::queued_operation(&intent.operation_id, 1);
    assert!(store
        .accept_thread_post("owner-fenced", &plan.agents, &receipt)
        .is_err());
    assert!(store.operation(&intent.operation_id).unwrap().is_none());
    assert!(store.thread_items(&plan.agents[0].id).unwrap().is_empty());
    assert!(store.delivery_intents_ready(1).unwrap().is_empty());
    store
        .edit_waiting_on(PROJECT, &task.id, &[], &[blocker.id], &Actor::User, NOW)
        .unwrap();
    store
        .fail_delivery_intent(&intent.id, intent.generation, "target unavailable")
        .unwrap();
    assert!(store
        .accept_thread_post("owner-fenced", &plan.agents, &receipt)
        .is_err());
    store.retry_delivery_intent(&intent.id).unwrap();
    store
        .accept_thread_post("owner-fenced", &plan.agents, &receipt)
        .unwrap();
    let mut unassigned = task.clone();
    unassigned.assignee = None;
    store
        .save_tracker_task_activity(&unassigned, &[], &[])
        .unwrap();
    assert_eq!(
        store
            .accept_thread_post("owner-fenced", &plan.agents, &receipt)
            .unwrap(),
        receipt,
        "a revoked generation can reconcile its prior receipt without posting again"
    );
}
