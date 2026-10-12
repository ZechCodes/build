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

fn freeze_post(
    store: &Store,
    task: &Task,
    intent: &DeliveryIntent,
    owner: &str,
) -> (PersistedPlan, crate::operation::OperationReceipt) {
    let mut plan = super::support::plan_record(owner);
    let agent = &mut plan.agents[0];
    agent.thread.post_user("frozen assignment", None, NOW);
    let envelope = crate::thread::TaskEnvelope {
        task_id: task.id.clone(),
        number: task.number,
        title: task.title.clone(),
        links: task.links.clone(),
    };
    let crate::thread::ThreadItem::Message(message) = agent.thread.items.last_mut().unwrap() else {
        panic!("fixture posted a message");
    };
    message.from_task = Some(Box::new(envelope.clone()));
    let message = message.clone();
    let mut receipt = super::support::queued_operation(&intent.operation_id, 1);
    receipt.entity_id = owner.into();
    receipt.agent_id = agent.id.clone();
    receipt.conversation_id = agent.conversation_id().into();
    let params = json!({"entity_id":owner,"agent_id":receipt.agent_id,
        "conversation_id":receipt.conversation_id,"thread_id":agent.thread.id,
        "body":"frozen assignment","operation_id":intent.operation_id});
    receipt.request_hash = crate::operation::thread_post_request_hash(
        &params,
        owner,
        &receipt.agent_id,
        &receipt.conversation_id,
    );
    let delivery = receipt.delivery.as_mut().unwrap();
    delivery.owner_id = owner.into();
    delivery.agent_id = receipt.agent_id.clone();
    delivery.payload.as_mut().unwrap().messages = vec![message];
    store
        .checkpoint_delivery_intent(
            &intent.id,
            intent.generation,
            json!({}),
            Some(json!({
                "params":params,"envelope":envelope,"sender":null,"requester":null,
                "intent_id":intent.id,"generation":intent.generation,
            })),
        )
        .unwrap();
    (plan, receipt)
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
    let (plan, receipt) = freeze_post(&store, &task, &intent, "owner-delivery");
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
    let (plan, receipt) = freeze_post(&store, &task, &intent, "owner-fenced");
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

#[test]
fn delivery_admission_binds_frozen_payload_address_and_origin() {
    for changed in [
        "body",
        "owner",
        "agent",
        "conversation",
        "method",
        "requester",
        "envelope",
        "sender",
    ] {
        refused_frozen_change(changed);
    }
}

fn refused_frozen_change(changed: &str) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let (task, intent) = assignment(&store);
    store
        .claim_delivery_intent(&intent.id, intent.generation)
        .unwrap();
    let (plan, original) = freeze_post(&store, &task, &intent, "owner-origin");
    let mut forged = original.clone();
    forge_frozen_receipt(&mut forged, changed);
    assert!(
        store
            .accept_thread_post("owner-origin", &plan.agents, &forged)
            .is_err(),
        "{changed} bypassed immutable admission"
    );
    assert!(store.operation(&intent.operation_id).unwrap().is_none());
    assert!(store.thread_items(&plan.agents[0].id).unwrap().is_empty());
    store
        .accept_thread_post("owner-origin", &plan.agents, &original)
        .unwrap();
    assert_eq!(
        store.operation(&intent.operation_id).unwrap().unwrap(),
        original
    );
}

fn forge_frozen_receipt(receipt: &mut crate::operation::OperationReceipt, changed: &str) {
    match changed {
        "body" => receipt.request_hash = "hash of caller substituted body".into(),
        "owner" => receipt.entity_id = "foreign-owner".into(),
        "agent" => receipt.agent_id = "foreign-agent".into(),
        "conversation" => receipt.conversation_id = "foreign-conversation".into(),
        "method" => receipt.method = "forged.method".into(),
        "requester" => {
            receipt.requested_by = Some(crate::operation::OperationRequester {
                entity_id: "foreign-sender-owner".into(),
                agent_id: "foreign-sender".into(),
                conversation_id: "foreign-sender-conversation".into(),
            })
        }
        other => forge_message_origin(receipt, other),
    }
}

fn forge_message_origin(receipt: &mut crate::operation::OperationReceipt, changed: &str) {
    let message = &mut receipt
        .delivery
        .as_mut()
        .unwrap()
        .payload
        .as_mut()
        .unwrap()
        .messages[0];
    match changed {
        "envelope" => {
            message.from_task.as_mut().unwrap().title = "substituted task envelope".into()
        }
        "sender" => {
            message.from_agent = Some(Box::new(crate::thread::AgentIdentity::new("forged-sender")))
        }
        _ => unreachable!("unknown forged field"),
    }
}

#[test]
fn moving_pending_creating_assignment_from_backlog_to_ready_retires_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = store
        .create_tracker_task(
            Task::drafted(PROJECT, "waiting for an agent", Actor::User, NOW),
            &[],
        )
        .unwrap();
    let mut blocker = store
        .create_tracker_task(
            Task::drafted(PROJECT, "prerequisite", Actor::User, NOW),
            &[],
        )
        .unwrap();
    store
        .edit_waiting_on(
            PROJECT,
            &task.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            NOW,
        )
        .unwrap();
    let assigned = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Assigned,
        json!({}),
        NOW,
    );
    let intent = store
        .accept_delivery_intent(
            &task,
            &[assigned],
            DeliveryIntent {
                id: String::new(),
                task_id: task.id.clone(),
                project_path: PROJECT.into(),
                generation: 0,
                target: json!({"kind":"new_agent","workspace_id":"existing-workspace"}),
                actor: Actor::User,
                sender: None,
                note: None,
                operation_id: String::new(),
                state: String::new(),
                reservation: serde_json::Value::Null,
                admitted: None,
                error: None,
            },
        )
        .unwrap();
    assert_eq!(intent.state, "waiting");
    assert!(task.assignee.is_none());
    task.status = "ready".into();
    let moved = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        json!({"from":"backlog","to":"ready"}),
        NOW,
    );
    store
        .save_tracker_task_activity(&task, &[], &[moved])
        .unwrap();
    blocker.status = "done".into();
    store
        .save_tracker_task_activity(&blocker, &[], &[])
        .unwrap();
    while store.reconcile_waiting_on(100).unwrap().pending {}
    assert_eq!(
        store
            .load_delivery_intent(&intent.id)
            .unwrap()
            .unwrap()
            .state,
        "cancelled"
    );
    assert!(!store
        .delivery_authority_current(&intent.id, intent.generation)
        .unwrap());
    assert!(store.delivery_intents_ready(100).unwrap().is_empty());
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().status,
        "ready"
    );
}
