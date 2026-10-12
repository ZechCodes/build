use super::project_agent::{project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::tracker::Actor;

fn waiting_task(app: &mut AppState, project_id: &str) -> (String, String) {
    let task = filed(app, project_id, "dependent");
    let blocker = filed(app, project_id, "prerequisite");
    let id = task["id"].as_str().unwrap().to_owned();
    let blocker_id = blocker["id"].as_str().unwrap().to_owned();
    let path = app.tracker_project_path(project_id).unwrap();
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &path,
            &id,
            std::slice::from_ref(&blocker_id),
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();
    while app
        .tracker_store()
        .unwrap()
        .reconcile_waiting_on(2)
        .unwrap()
        .pending
    {}
    (id, blocker_id)
}

fn assign_waiting(app: &mut AppState, id: &str, target: Value) -> Value {
    let answer = app.handle(req(
        "tasks.assign",
        json!({"task_id":id,"assignee":target,"note":"original note"}),
    ));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

#[test]
fn waiting_assignment_defers_every_target() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (_owner, pin) = project_agent(&mut app, &project);
    let ws = workspace(&mut app, &project, "existing");
    let targets = [
        json!({"kind":"agent","agent_id":pin}),
        json!({"kind":"project_agent"}),
        json!({"kind":"new_agent","workspace_id":ws}),
        json!({"kind":"new_workspace","name":"late checkout"}),
        json!({"kind":"new_project_agent"}),
    ];
    for target in targets {
        let (id, _) = waiting_task(&mut app, &project);
        let before_workspaces = app.workspaces.list(Some(&project)).len();
        let before_runs = app.runs.len();
        let answer = assign_waiting(&mut app, &id, target.clone());
        assert!(
            answer["dispatch"].is_null(),
            "waiting delivered {target}: {answer}"
        );
        assert_eq!(answer["task"]["status"], "backlog");
        assert_eq!(
            answer["task"]["deferred_assignment"]["state"], "waiting",
            "{answer}"
        );
        assert_eq!(app.workspaces.list(Some(&project)).len(), before_workspaces);
        assert_eq!(app.runs.len(), before_runs);
        assert!(app.deferred_work.is_none());
        let timeline = app
            .tracker_store()
            .unwrap()
            .load_tracker_timeline(&id)
            .unwrap();
        assert!(!timeline.iter().any(|entry| matches!(entry, crate::tracker::TimelineEntry::Event(event) if event.kind == crate::tracker::TaskEventKind::Dispatched)));
    }
    for target in [json!({"kind":"user"}), Value::Null] {
        let (id, _) = waiting_task(&mut app, &project);
        let answer = assign_waiting(&mut app, &id, target);
        assert!(answer["dispatch"].is_null());
        assert!(answer["task"]["deferred_assignment"].is_null());
    }
}

fn finish(app: &mut AppState, id: &str) {
    let answer = app.handle(req("tasks.update", json!({"task_id":id,"status":"done"})));
    assert_eq!(answer["ok"], true, "{answer}");
}

fn pump(app: &mut AppState) {
    for _ in 0..1000 {
        if !app.pump_task_deliveries_for_test().unwrap() {
            return;
        }
    }
    panic!("bounded delivery service failed to drain");
}

fn dispatched_count(app: &AppState, id: &str) -> usize {
    app.tracker_store().unwrap().load_tracker_timeline(id).unwrap().iter()
        .filter(|entry| matches!(entry, crate::tracker::TimelineEntry::Event(event) if event.kind == crate::tracker::TaskEventKind::Dispatched)).count()
}

#[test]
fn unblock_fan_in_and_fan_out() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (_owner, pin) = project_agent(&mut app, &project);
    let path = app.tracker_project_path(&project).unwrap();
    let shared_blocker = filed(&mut app, &project, "shared blocker")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut dependents = Vec::new();
    for _ in 0..103 {
        let (id, other) = waiting_task(&mut app, &project);
        app.tracker_store()
            .unwrap()
            .edit_waiting_on(
                &path,
                &id,
                std::slice::from_ref(&shared_blocker),
                &[],
                &Actor::User,
                &crate::store::now_rfc3339(),
            )
            .unwrap();
        let answer = assign_waiting(&mut app, &id, json!({"kind":"agent","agent_id":pin}));
        assert!(answer["dispatch"].is_null(), "{answer}");
        finish(&mut app, &other);
        dependents.push(id);
    }
    pump(&mut app);
    assert!(dependents.iter().all(|id| dispatched_count(&app, id) == 0));
    finish(&mut app, &shared_blocker);
    pump(&mut app);
    assert!(dependents.iter().all(|id| dispatched_count(&app, id) == 1));
    let reopened = app.handle(req(
        "tasks.update",
        json!({"task_id":shared_blocker,"status":"ready"}),
    ));
    assert_eq!(reopened["ok"], true, "{reopened}");
    pump(&mut app);
    finish(&mut app, &shared_blocker);
    pump(&mut app);
    assert!(
        dependents.iter().all(|id| dispatched_count(&app, id) == 1),
        "consumed assignments must never redispatch"
    );
}

#[test]
fn deferred_dispatch_generation_fences() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (_owner, pin) = project_agent(&mut app, &project);
    for cancellation in ["unassign", "user", "close", "done"] {
        let (id, blocker) = waiting_task(&mut app, &project);
        let answer = assign_waiting(&mut app, &id, json!({"kind":"agent","agent_id":pin}));
        assert!(answer["dispatch"].is_null(), "{answer}");
        let changed = match cancellation {
            "unassign" => app.handle(req("tasks.assign", json!({"task_id":id,"assignee":null}))),
            "user" => app.handle(req(
                "tasks.assign",
                json!({"task_id":id,"assignee":{"kind":"user"}}),
            )),
            "close" => app.handle(req("tasks.close", json!({"task_id":id}))),
            _ => app.handle(req("tasks.update", json!({"task_id":id,"status":"done"}))),
        };
        assert_eq!(changed["ok"], true, "{changed}");
        finish(&mut app, &blocker);
        pump(&mut app);
        assert_eq!(
            dispatched_count(&app, &id),
            0,
            "stale {cancellation} delivered"
        );
    }
    let (id, blocker) = waiting_task(&mut app, &project);
    assign_waiting(&mut app, &id, json!({"kind":"agent","agent_id":pin}));
    let first = app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&id)
        .unwrap()
        .unwrap();
    assign_waiting(&mut app, &id, json!({"kind":"new_project_agent"}));
    assert!(!app
        .tracker_store()
        .unwrap()
        .delivery_authority_current(&first.id, first.generation)
        .unwrap());
    finish(&mut app, &blocker);
    pump(&mut app);
    assert_eq!(dispatched_count(&app, &id), 1);
}

#[test]
fn deferred_dispatch_restart_boundaries() {
    use crate::app::tracker::DeferredDeliveryBoundary::*;
    for boundary in [Reserved, Materialized, Agent, Frozen, Posted, Settled] {
        restart_at(boundary);
    }
}

fn restart_at(boundary: crate::app::tracker::DeferredDeliveryBoundary) {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (id, blocker) = waiting_task(&mut app, &project);
    let answer = assign_waiting(
        &mut app,
        &id,
        json!({"kind":"new_workspace","name":"one owned checkout","agent_name":"one worker"}),
    );
    assert!(answer["dispatch"].is_null(), "{answer}");
    let intent = app
        .tracker_store()
        .unwrap()
        .current_delivery_intent(&id)
        .unwrap()
        .unwrap();
    finish(&mut app, &blocker);
    app.stop_task_delivery_after_for_test(boundary);
    assert!(app.pump_task_deliveries_for_test().unwrap());
    drop(app);
    let mut app = super::project_agent::rooted(root.path())
        .with_task_store(root.path().join("store"))
        .unwrap();
    pump(&mut app);
    let stored = app
        .tracker_store()
        .unwrap()
        .load_delivery_intent(&intent.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, "delivered", "{boundary:?}: {stored:?}");
    assert_eq!(stored.operation_id, intent.operation_id);
    assert_eq!(app.workspaces.list(Some(&project)).len(), 1, "{boundary:?}");
    assert_eq!(dispatched_count(&app, &id), 1, "{boundary:?}");
    let receipt = app
        .tracker_store()
        .unwrap()
        .operation(&intent.operation_id)
        .unwrap()
        .unwrap();
    let page = app.handle(req(
        "thread.page",
        json!({"entity_id":receipt.entity_id,"agent_id":receipt.agent_id,"limit":100}),
    ));
    let messages = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message" && item["data"]["from_task"]["task_id"] == id)
        .count();
    assert_eq!(messages, 1, "{boundary:?}: {page}");
    pump(&mut app);
    assert_eq!(dispatched_count(&app, &id), 1);
}
