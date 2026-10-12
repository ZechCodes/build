//! Regression evidence for immutable admission, ownership, and client-free work.

use super::project_agent::{added_project, project_agent, rooted, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::app::tracker::DeferredDeliveryBoundary;
use crate::store::DeliveryIntent;
use crate::tracker::{Actor, TaskEventKind, TimelineEntry};

struct WaitingDelivery {
    root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    app: AppState,
    project: String,
    owner: String,
    pin: String,
    task: String,
    blocker: String,
}

impl WaitingDelivery {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (repo, mut app, project) = tracked(root.path());
        let (owner, pin) = project_agent(&mut app, &project);
        let task = filed(&mut app, &project, "original dependent")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let blocker = filed(&mut app, &project, "original prerequisite")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let mut fixture = Self {
            root,
            _repo: repo,
            app,
            project,
            owner,
            pin,
            task,
            blocker,
        };
        fixture.edges(std::slice::from_ref(&fixture.blocker.clone()), &[]);
        fixture.reconcile();
        fixture
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let answer = self.app.handle(req(method, params));
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"].clone()
    }

    fn edges(&mut self, add: &[String], remove: &[String]) {
        let path = self.app.tracker_project_path(&self.project).unwrap();
        self.app
            .tracker_store()
            .unwrap()
            .edit_waiting_on(
                &path,
                &self.task,
                add,
                remove,
                &Actor::User,
                &crate::store::now_rfc3339(),
            )
            .unwrap();
    }

    fn reconcile(&self) {
        while self
            .app
            .tracker_store()
            .unwrap()
            .reconcile_waiting_on(100)
            .unwrap()
            .pending
        {}
    }

    fn assign(&mut self, target: Value) -> DeliveryIntent {
        let result = self.call(
            "tasks.assign",
            json!({
                "task_id": self.task, "assignee": target, "note": "original instruction",
            }),
        );
        assert!(result["dispatch"].is_null(), "{result}");
        self.app
            .tracker_store()
            .unwrap()
            .current_delivery_intent(&self.task)
            .unwrap()
            .unwrap()
    }

    fn release(&mut self) {
        self.call(
            "tasks.update",
            json!({"task_id":self.blocker,"status":"done"}),
        );
    }

    fn pump(&mut self) {
        for _ in 0..1000 {
            if !self.app.pump_task_deliveries_for_test().unwrap() {
                return;
            }
        }
        panic!("delivery service did not drain bounded fixture");
    }

    fn frozen(&mut self, target: Value) -> DeliveryIntent {
        let intent = self.assign(target);
        self.release();
        self.app
            .stop_task_delivery_after_for_test(DeferredDeliveryBoundary::Frozen);
        assert!(self.app.pump_task_deliveries_for_test().unwrap());
        let frozen = self.load(&intent.id);
        assert!(frozen.admitted.is_some(), "{frozen:?}");
        assert!(self
            .app
            .tracker_store()
            .unwrap()
            .operation(&intent.operation_id)
            .unwrap()
            .is_none());
        frozen
    }

    fn load(&self, id: &str) -> DeliveryIntent {
        self.app
            .tracker_store()
            .unwrap()
            .load_delivery_intent(id)
            .unwrap()
            .unwrap()
    }

    fn event_count(&self, kind: TaskEventKind) -> usize {
        self.app
            .tracker_store()
            .unwrap()
            .load_tracker_timeline(&self.task)
            .unwrap()
            .iter()
            .filter(|item| matches!(item, TimelineEntry::Event(event) if event.kind == kind))
            .count()
    }

    fn messages(&mut self, owner: &str, agent: &str) -> Vec<Value> {
        self.call(
            "thread.page",
            json!({"entity_id":owner,"agent_id":agent,"limit":100}),
        )["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "message")
            .map(|item| item["data"].clone())
            .filter(|message| message["from_task"]["task_id"] == self.task)
            .collect()
    }

    fn worker(&mut self) -> String {
        self.call(
            "agent.add",
            json!({"entity_id":self.owner,"project_role":"worker"}),
        )["agent"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn remove_agent(&mut self, agent: &str) {
        self.call(
            "agent.remove",
            json!({"entity_id":self.owner,"agent_id":agent}),
        );
    }
}

#[test]
fn app_deferred_dispatch_retry_freezes_mutable_message_and_context() {
    let mut fixture = WaitingDelivery::new();
    let closed = filed(
        &mut fixture.app,
        &fixture.project,
        "original closed prerequisite",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    fixture.call("tasks.close", json!({"task_id":closed}));
    fixture.edges(std::slice::from_ref(&closed), &[]);
    let old_workspace = workspace(
        &mut fixture.app,
        &fixture.project,
        "original linked checkout",
    );
    let new_workspace = workspace(&mut fixture.app, &fixture.project, "edited linked checkout");
    let linked = link_task(
        &mut fixture.app,
        json!({"task_id":fixture.task,"workspace_id":old_workspace}),
    );
    assert_eq!(linked["ok"], true, "{linked}");
    let frozen = fixture.frozen(json!({"kind":"agent","agent_id":fixture.pin}));
    let admitted = frozen.admitted.clone().unwrap();
    let body = admitted["params"]["body"].as_str().unwrap().to_string();
    assert!(body.contains("original closed prerequisite"), "{body}");
    fixture
        .app
        .tracker_store()
        .unwrap()
        .fail_delivery_intent(
            &frozen.id,
            frozen.generation,
            "injected failure before post",
        )
        .unwrap();
    mutate_after_freeze(&mut fixture, &closed, &new_workspace);
    fixture.app.retry_task_delivery(&frozen.id).unwrap();
    assert_eq!(fixture.load(&frozen.id).admitted, Some(admitted.clone()));
    let root = fixture.root.path().to_path_buf();
    drop(fixture.app);
    fixture.app = rooted(&root).with_task_store(root.join("store")).unwrap();
    fixture.pump();
    let delivered = fixture.load(&frozen.id);
    assert_eq!(delivered.state, "delivered");
    assert_eq!(delivered.operation_id, frozen.operation_id);
    assert_eq!(delivered.admitted, Some(admitted));
    let messages = fixture.messages(&fixture.owner.clone(), &fixture.pin.clone());
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0]["body"].as_str().unwrap(), body);
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 1);
}

fn mutate_after_freeze(fixture: &mut WaitingDelivery, closed: &str, workspace: &str) {
    let store = fixture.app.tracker_store().unwrap();
    let mut task = store.load_tracker_task(&fixture.task).unwrap().unwrap();
    task.title = "edited dependent title".into();
    task.body = "edited task body must not rebuild admitted request".into();
    task.links.workspace_ids = vec![workspace.into()];
    task.links.branches = vec!["edited-branch".into()];
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    fixture.call(
        "tasks.update",
        json!({"task_id":closed,"title":"edited closed prerequisite"}),
    );
    fixture.edges(&[], &[closed.into()]);
    let later = filed(
        &mut fixture.app,
        &fixture.project,
        "newly closed prerequisite",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    fixture.call("tasks.close", json!({"task_id":later}));
    fixture.edges(&[later], &[]);
    fixture.reconcile();
}

#[test]
fn deferred_delivery_revalidates_foreign_target_before_admission() {
    let mut fixture = WaitingDelivery::new();
    let worker = fixture.worker();
    let frozen = fixture.frozen(json!({"kind":"agent","agent_id":worker}));
    let repo = init_repo_named(fixture.root.path(), "foreign-owner");
    let foreign = added_project(&mut fixture.app, &repo);
    // Model restored ownership changing after the slow provisioning boundary.
    fixture
        .app
        .projects
        .bind_entity(fixture.owner.clone(), foreign);
    fixture.pump();
    assert_eq!(fixture.load(&frozen.id).state, "failed");
    assert!(fixture
        .app
        .tracker_store()
        .unwrap()
        .operation(&frozen.operation_id)
        .unwrap()
        .is_none());
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 0);
}

#[test]
fn deferred_delivery_operation_id_cannot_admit_a_forged_payload_or_foreign_owner() {
    let mut fixture = WaitingDelivery::new();
    let frozen = fixture.frozen(json!({"kind":"agent","agent_id":fixture.pin}));
    let repo = init_repo_named(fixture.root.path(), "foreign-post");
    let foreign = added_project(&mut fixture.app, &repo);
    let (foreign_owner, foreign_pin) = project_agent(&mut fixture.app, &foreign);
    let admitted = frozen.admitted.as_ref().unwrap();
    let mut changed_body = admitted["params"].clone();
    changed_body["body"] = json!("caller substituted an instruction");
    let mut changed_owner = admitted["params"].clone();
    changed_owner["entity_id"] = json!(foreign_owner);
    changed_owner["agent_id"] = json!(foreign_pin);
    let address = fixture
        .app
        .resolve_conversation_address(&foreign_owner, Some(&foreign_pin))
        .unwrap();
    changed_owner["conversation_id"] = json!(address.conversation_id);
    // A fresh foreign conversation permits an unversioned mutation; keeping
    // the original generation would merely test the normal thread-id guard.
    changed_owner.as_object_mut().unwrap().remove("thread_id");
    for params in [changed_body, changed_owner] {
        let answer = fixture.app.handle(req("thread.post", params));
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(fixture
            .app
            .tracker_store()
            .unwrap()
            .operation(&frozen.operation_id)
            .unwrap()
            .is_none());
    }
    fixture.pump();
    assert_eq!(fixture.load(&frozen.id).state, "delivered");
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 1);
}

#[test]
fn deferred_delivery_removed_target_never_substitutes_another_agent() {
    let mut fixture = WaitingDelivery::new();
    let worker = fixture.worker();
    let intent = fixture.assign(json!({"kind":"agent","agent_id":worker}));
    fixture.remove_agent(&worker);
    let replacement = fixture.worker();
    assert_ne!(replacement, worker);
    fixture.release();
    fixture.pump();
    assert!(fixture
        .app
        .tracker_store()
        .unwrap()
        .operation(&intent.operation_id)
        .unwrap()
        .is_none());
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 0);
    assert!(fixture
        .messages(&fixture.owner.clone(), &replacement)
        .is_empty());
    assert_eq!(fixture.load(&intent.id).state, "failed");
}

#[test]
fn deferred_delivery_retired_sender_preserves_authority_failure() {
    let mut fixture = WaitingDelivery::new();
    let sender = fixture.worker();
    let result = fixture
        .app
        .on_agent_mcp_action(
            &fixture.owner,
            &sender,
            crate::mcp::BridgeAction::TrackerAssignTask {
                task_id: fixture.task.clone(),
                assignee: json!({"kind":"agent","agent_id":fixture.pin}),
                note: Some("sender instruction".into()),
                track: None,
                notify_user: Some(false),
            },
        )
        .unwrap();
    assert!(result["dispatch"].is_null(), "{result}");
    let intent = fixture
        .app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&fixture.task)
        .unwrap()
        .unwrap();
    assert!(matches!(&intent.actor, Actor::Agent { agent_id, .. } if agent_id == &sender));
    fixture.remove_agent(&sender);
    fixture.release();
    fixture.pump();
    let failed = fixture.load(&intent.id);
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.actor, intent.actor);
    assert!(fixture
        .app
        .tracker_store()
        .unwrap()
        .operation(&intent.operation_id)
        .unwrap()
        .is_none());
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 0);
}

#[test]
fn deferred_delivery_project_deletion_cancels_frozen_work_and_notices() {
    let mut fixture = WaitingDelivery::new();
    let frozen = fixture.frozen(json!({"kind":"new_project_agent"}));
    fixture.call(
        "project.delete",
        json!({"project_id":fixture.project,"confirm":true}),
    );
    fixture.pump();
    assert_eq!(fixture.load(&frozen.id).state, "cancelled");
    let store = fixture.app.tracker_store().unwrap();
    assert!(store.operation(&frozen.operation_id).unwrap().is_none());
    assert!(store.delivery_notices_ready(100).unwrap().is_empty());
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), 0);
    assert!(!fixture.app.runs.contains_key(&fixture.owner));
}

#[test]
fn removing_last_edge_delivers_armed_and_notifies_unassigned_without_rearming_cancelled() {
    for assignment in ["armed", "unassigned", "cancelled"] {
        edge_removal_case(assignment);
    }
}

fn edge_removal_case(assignment: &str) {
    let mut fixture = WaitingDelivery::new();
    if assignment != "unassigned" {
        fixture.assign(json!({"kind":"agent","agent_id":fixture.pin}));
    }
    if assignment == "cancelled" {
        fixture.call(
            "tasks.assign",
            json!({"task_id":fixture.task,"assignee":null}),
        );
    }
    fixture.edges(&[], &[fixture.blocker.clone()]);
    fixture.pump();
    let messages = fixture.messages(&fixture.owner.clone(), &fixture.pin.clone());
    let dispatches = fixture.event_count(TaskEventKind::Dispatched);
    assert_eq!(
        dispatches,
        usize::from(assignment == "armed"),
        "{assignment}"
    );
    assert_eq!(messages.len(), 1, "{assignment}: {messages:?}");
    let body = messages[0]["body"].as_str().unwrap();
    assert!(body.contains("removed"), "{assignment}: {body}");
    let store = fixture.app.tracker_store().unwrap();
    let task = store.load_tracker_task(&fixture.task).unwrap().unwrap();
    if assignment != "armed" {
        assert!(task.assignee.is_none());
        assert_eq!(task.status, "backlog");
    }
    let timeline = store.load_tracker_timeline(&fixture.task).unwrap();
    let unblocked: Vec<_> = timeline
        .iter()
        .filter_map(|item| match item {
            TimelineEntry::Event(event) if event.kind == TaskEventKind::Unblocked => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(unblocked.len(), 1);
    assert_eq!(
        unblocked[0].payload["removed_task_ids"],
        json!([fixture.blocker])
    );
    fixture.pump();
    assert_eq!(fixture.event_count(TaskEventKind::Dispatched), dispatches);
}

#[tokio::test]
async fn deferred_delivery_startup_runs_without_rpc_or_connected_client() {
    let mut fixture = WaitingDelivery::new();
    let intent = fixture.assign(json!({"kind":"new_project_agent","agent_name":"startup worker"}));
    fixture.release();
    let root = fixture.root.path().to_path_buf();
    drop(fixture.app);
    let app = rooted(&root)
        .with_task_store(root.join("store"))
        .unwrap()
        .shared();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let delivered = {
                let state = app.lock().unwrap();
                state
                    .tracker_store()
                    .unwrap()
                    .load_delivery_intent(&intent.id)
                    .unwrap()
                    .unwrap()
                    .state
                    == "delivered"
            };
            if delivered {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup must discover queued work without any client request or subscription");
    await_startup_provider_handoff(&app, &intent.operation_id).await;
    let app = app.lock().unwrap();
    let receipt = app
        .tracker_store()
        .unwrap()
        .operation(&intent.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, crate::operation::OperationStatus::Delivered);
    assert_ne!(receipt.agent_id, fixture.pin);
    let task = app
        .tracker_store()
        .unwrap()
        .load_tracker_task(&fixture.task)
        .unwrap()
        .unwrap();
    assert_eq!(task.status, "in_progress");
    assert_eq!(app.entity_agents(&fixture.owner).unwrap().len(), 2);
}

async fn await_startup_provider_handoff(app: &Arc<Mutex<AppState>>, operation: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let handed_off = app
                .lock()
                .unwrap()
                .tracker_store()
                .unwrap()
                .operation(operation)
                .unwrap()
                .is_some_and(|receipt| {
                    receipt.status == crate::operation::OperationStatus::Delivered
                });
            if handed_off {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("client-free startup must hand the accepted brief to the QA provider session");
}
