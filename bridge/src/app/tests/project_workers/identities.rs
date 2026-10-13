//! A worker's durable purpose does not depend on its optional name or whether
//! it is still present on the project roster.

use super::*;
use crate::mcp::BridgeAction;
use crate::tracker::Assignee;

fn unnamed_worker_task(project: &mut WorkerProject) -> (String, String) {
    let worker = add_worker(
        &mut project.state,
        &project.owner,
        json!({ "provider": "codex" }),
    );
    let task = super::super::tracker::filed(
        &mut project.state,
        &project.project_id,
        "An unnamed worker's history",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Record an assignment without launching a harness.
    let store = project.state.tracker_store().unwrap();
    let mut saved = store.load_tracker_task(&task).unwrap().unwrap();
    saved.assignee = Some(Assignee::Agent {
        agent_id: worker.clone(),
    });
    saved.body = format!("The coordinator is @agent:{}.", project.pin);
    store.save_tracker_task_activity(&saved, &[], &[]).unwrap();
    project
        .state
        .on_agent_mcp_action(
            &project.owner,
            &worker,
            BridgeAction::TrackerCommentTask {
                task_id: task.clone(),
                body: "An unnamed worker wrote this.".into(),
                refs: Vec::new(),
                track: Some(false),
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .unwrap();
    (worker, task)
}

fn assert_worker_identity(identity: &Value, worker: &str, available: bool) {
    assert_eq!(identity["agent_id"], worker);
    assert_eq!(identity["name"], Value::Null);
    assert_eq!(identity["project_role"], "worker");
    assert_eq!(identity["provider"], "codex");
    assert_eq!(identity["ordinal"], 2);
    assert_eq!(identity["available"], available);
}

#[test]
fn unnamed_project_worker_identity_survives_removal() {
    let mut project = WorkerProject::new();
    let (worker, task) = unnamed_worker_task(&mut project);
    let removed = project.state.handle(req(
        "agent.remove",
        json!({ "entity_id": project.owner, "agent_id": worker }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    let read = project
        .state
        .handle(req("tasks.get", json!({ "task_id": task })));
    let identity = &read["result"]["task"]["identities"][&worker];
    assert_worker_identity(identity, &worker, false);
    assert_eq!(
        read["result"]["task"]["identities"][&project.pin]["project_role"],
        "coordinator"
    );
    let listed = project.state.handle(req(
        "tasks.list",
        json!({ "project_id": project.project_id }),
    ));
    assert_eq!(
        listed["result"]["tasks"][0]["identities"][&worker],
        *identity
    );
    let saved = project
        .state
        .tracker_store()
        .unwrap()
        .load_tracker_task(&task)
        .unwrap()
        .unwrap();
    assert_worker_identity(&json!(saved.identities[&worker]), &worker, true);
}

#[test]
fn stored_project_worker_purpose_backfills_older_task_identities() {
    let mut project = WorkerProject::new();
    let (worker, task) = unnamed_worker_task(&mut project);
    let store = project.state.tracker_store().unwrap();
    let mut saved = store.load_tracker_task(&task).unwrap().unwrap();
    saved.identities.clear();
    store.save_tracker_task_activity(&saved, &[], &[]).unwrap();
    // The roster is now available only from the durable run, and this task
    // predates identity snapshots. Reading it must restore the worker's role.
    project.state.runs.remove(&project.owner);
    let read = project
        .state
        .handle(req("tasks.get", json!({ "task_id": task })));
    let identity = &read["result"]["task"]["identities"][&worker];
    assert_worker_identity(identity, &worker, false);
    assert_eq!(identity["workspace_id"], Value::Null);
    assert_eq!(identity["workspace_name"], Value::Null);
    let saved = project
        .state
        .tracker_store()
        .unwrap()
        .load_tracker_task(&task)
        .unwrap()
        .unwrap();
    let saved_identity = json!(saved.identities[&worker]);
    assert_worker_identity(&saved_identity, &worker, false);
    assert_eq!(saved_identity["workspace_id"], Value::Null);
    assert_eq!(saved_identity["workspace_name"], Value::Null);
}

#[test]
fn stored_project_purpose_removes_legacy_workspace_identity_fields() {
    let mut project = WorkerProject::new();
    let (worker, task) = unnamed_worker_task(&mut project);
    let store = project.state.tracker_store().unwrap();
    let mut saved = store.load_tracker_task(&task).unwrap().unwrap();
    for agent in [&worker, &project.pin] {
        let identity = saved.identities.get_mut(agent).unwrap();
        identity.project_role = None;
        identity.workspace_id = Some("legacy-synthetic-workspace".into());
        identity.workspace_name = Some("project-scratch".into());
    }
    store.save_tracker_task_activity(&saved, &[], &[]).unwrap();
    project.state.runs.remove(&project.owner);
    let read = project
        .state
        .handle(req("tasks.get", json!({ "task_id": task })));
    assert_worker_identity(
        &read["result"]["task"]["identities"][&worker],
        &worker,
        false,
    );
    let saved = project
        .state
        .tracker_store()
        .unwrap()
        .load_tracker_task(&task)
        .unwrap()
        .unwrap();
    for agent in [&worker, &project.pin] {
        let identity = &read["result"]["task"]["identities"][agent];
        assert_eq!(identity["workspace_id"], Value::Null);
        assert_eq!(identity["workspace_name"], Value::Null);
        assert_eq!(saved.identities[agent].workspace_id, None);
        assert_eq!(saved.identities[agent].workspace_name, None);
    }
    assert_eq!(
        read["result"]["task"]["identities"][&project.pin]["project_role"],
        "coordinator"
    );
}

#[test]
fn inline_task_notice_names_an_unnamed_project_worker_by_its_purpose() {
    let mut project = WorkerProject::new();
    let (worker, task) = unnamed_worker_task(&mut project);
    set_task_tracking(&mut project.state, &task, &project.pin, true);
    project
        .state
        .on_agent_mcp_action(
            &project.owner,
            &worker,
            BridgeAction::TrackerMoveTask {
                task_id: task,
                status: "in_review".into(),
                track: Some(false),
            },
        )
        .unwrap();
    let items =
        super::super::project_agent::items(&mut project.state, &project.owner, &project.pin);
    let notice = items
        .iter()
        .find(|item| item["data"]["task_notice"]["action"] == "moved")
        .unwrap();
    assert_worker_identity(
        &notice["data"]["task_notice"]["actor"]["identity"],
        &worker,
        true,
    );
}
