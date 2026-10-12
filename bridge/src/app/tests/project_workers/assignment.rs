//! Assigning a task to a project worker, existing or new: one delivery to
//! the exact worker of the task's own project, and a retry that delivers
//! nothing twice.

use super::*;
use crate::mcp::BridgeAction;
use crate::tracker::{TaskEventKind, TimelineEntry};

fn file(project: &mut WorkerProject, title: &str) -> String {
    super::super::tracker::filed(&mut project.state, &project.project_id, title)["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn assign(state: &mut AppState, params: Value) -> Value {
    let answered = state.handle(req("tasks.assign", params));
    assert_eq!(answered["ok"], true, "{answered:?}");
    answered["result"].clone()
}

/// The operation ids of every `dispatched` event on one task, in order.
fn dispatches(state: &AppState, task_id: &str) -> Vec<String> {
    state
        .tracker_store()
        .unwrap()
        .load_tracker_timeline(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            TimelineEntry::Event(event) if event.kind == TaskEventKind::Dispatched => {
                event.payload["operation_id"].as_str().map(str::to_string)
            }
            _ => None,
        })
        .collect()
}

/// The task hand-offs delivered into one agent's conversation, leaving out
/// Build's own notices about the task.
fn notices(state: &mut AppState, entity_id: &str, agent_id: &str) -> usize {
    super::super::project_agent::items(state, entity_id, agent_id)
        .iter()
        .filter(|item| item["data"]["from_task"].is_object() && item["data"]["from_build"] != true)
        .count()
}

/// Handing a task to an existing worker, or to a new one, delivers it once
/// into that exact worker's conversation on the project's owner and links no
/// workspace. Asking again with the same `operation_id` answers the same
/// dispatch: no second delivery, no second worker. The same id cannot be
/// spent on a different assignment.
#[test]
fn assign_project_worker_is_idempotent() {
    let mut project = WorkerProject::new();
    let worker = project.worker("Rail scroll");
    let sibling = project.worker("Docs");
    let (owner, pin) = (project.owner.clone(), project.pin.clone());
    an_existing_worker_is_handed_a_task_once(&mut project, &worker);
    let fresh = a_new_worker_is_made_and_handed_a_task_once(&mut project);

    // One id, one assignment.
    let reused = project.state.handle(req(
        "tasks.assign",
        json!({
            "task_id": fresh,
            "assignee": { "kind": "agent", "agent_id": sibling },
            "operation_id": "op-assign-new",
        }),
    ));
    super::super::project_coordinator::refused_with(&reused, "conflict");
    assert_eq!(notices(&mut project.state, &owner, &sibling), 0);
    assert_eq!(notices(&mut project.state, &owner, &pin), 0);
    assert_eq!(project.state.project_pin(&owner), Some(pin.as_str()));
}

fn an_existing_worker_is_handed_a_task_once(project: &mut WorkerProject, worker: &str) {
    let owner = project.owner.clone();
    let existing = file(project, "Fix the rail");
    let request = json!({
        "task_id": existing,
        "assignee": { "kind": "agent", "agent_id": worker },
        "operation_id": "op-assign-existing",
    });
    let first = assign(&mut project.state, request.clone());
    assert_eq!(
        first["dispatch"],
        json!({
            "kind": "agent", "workspace_id": null, "entity_id": owner,
            "agent_id": worker, "operation_id": "op-assign-existing",
        })
    );
    assert_eq!(first["task"]["links"]["workspace_ids"], json!([]));
    assert_eq!(first["task"]["links"]["conversation_ids"], json!([owner]));
    let again = assign(&mut project.state, request);
    assert_eq!(again["dispatch"], first["dispatch"]);
    assert_eq!(again["task"]["assignee"], first["task"]["assignee"]);
    assert_eq!(
        dispatches(&project.state, &existing),
        vec!["op-assign-existing"]
    );
    assert_eq!(notices(&mut project.state, &owner, worker), 1);
}

/// Answers the task's id.
fn a_new_worker_is_made_and_handed_a_task_once(project: &mut WorkerProject) -> String {
    let owner = project.owner.clone();
    let fresh = file(project, "Write the docs page");
    let request = json!({
        "task_id": fresh,
        "assignee": {
            "kind": "new_project_agent", "agent_name": "Docs page",
            "provider": "codex", "effort": "high",
        },
        "operation_id": "op-assign-new",
    });
    let made = assign(&mut project.state, request.clone());
    let new_worker = made["dispatch"]["agent_id"].as_str().unwrap().to_string();
    assert_eq!(made["dispatch"]["kind"], "new_project_agent");
    assert_eq!(made["dispatch"]["entity_id"], json!(owner));
    assert_eq!(made["dispatch"]["workspace_id"], Value::Null);
    assert_eq!(
        made["task"]["assignee"],
        json!({ "kind": "agent", "agent_id": new_worker })
    );
    let added = project
        .state
        .entity_agents(&owner)
        .unwrap()
        .by_id(&new_worker)
        .unwrap()
        .clone();
    assert_eq!(
        added.project_role,
        Some(crate::project_roster::ProjectRole::Worker)
    );
    assert_eq!(added.name.as_deref(), Some("Docs page"));
    assert_eq!(added.choice.provider, crate::models::AgentProvider::Codex);
    let roster = project.roster();
    let replayed = assign(&mut project.state, request);
    assert_eq!(replayed["dispatch"], made["dispatch"]);
    assert_eq!(project.roster(), roster, "no second worker");
    assert_eq!(dispatches(&project.state, &fresh), vec!["op-assign-new"]);
    assert_eq!(notices(&mut project.state, &owner, &new_worker), 1);
    fresh
}

/// A task reaches only the agents of its own project: another project's
/// worker and a removed worker are refused, and nothing is delivered.
#[test]
fn assigning_refuses_a_foreign_or_removed_project_worker() {
    let mut project = WorkerProject::new();
    let removed = project.worker("Gone");
    let owner = project.owner.clone();
    let task = file(&mut project, "Somebody else's");

    let (_other_home, other_repo) = a_repo();
    let other_id = added_project(&mut project.state, &other_repo);
    let other_owner = ensure(&mut project.state, &other_id);
    let foreign = add_worker(&mut project.state, &other_owner, json!({}));
    let refused = project.state.handle(req(
        "tasks.assign",
        json!({ "task_id": task, "assignee": { "kind": "agent", "agent_id": foreign } }),
    ));
    super::super::project_coordinator::refused_with(&refused, "conflict");
    assert_eq!(notices(&mut project.state, &other_owner, &foreign), 0);

    let gone = project.state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": removed }),
    ));
    assert_eq!(gone["ok"], true, "{gone:?}");
    let refused = project.state.handle(req(
        "tasks.assign",
        json!({ "task_id": task, "assignee": { "kind": "agent", "agent_id": removed } }),
    ));
    super::super::project_coordinator::refused_with(&refused, "not_found");
    assert!(dispatches(&project.state, &task).is_empty());
}

/// A worker hands work to a new worker through `assign_task` the way the
/// pin does: same project, unwatched, stamped with who made it.
#[test]
fn a_worker_assigns_a_task_to_a_new_project_worker() {
    let mut project = WorkerProject::new();
    let worker = project.worker("Lead");
    let owner = project.owner.clone();
    let task = file(&mut project, "Split the parser");

    let assigned = project
        .state
        .on_agent_mcp_action(
            &owner,
            &worker,
            BridgeAction::TrackerAssignTask {
                task_id: task.clone(),
                assignee: json!({ "kind": "new_project_agent", "agent_name": "Parser", "harness": "codex" }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .unwrap();
    let made = assigned["dispatch"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(assigned["dispatch"]["entity_id"], json!(owner));
    let agent = project
        .state
        .entity_agents(&owner)
        .unwrap()
        .by_id(&made)
        .unwrap()
        .clone();
    assert_eq!(agent.created_by.as_deref(), Some(worker.as_str()));
    assert!(!agent.watched, "an agent's new worker is unwatched");
    assert_eq!(agent.choice.provider, crate::models::AgentProvider::Codex);
    assert_eq!(notices(&mut project.state, &owner, &made), 1);
}

/// A worker's session authenticated before its conversation was cleared
/// cannot hand a task to anybody afterwards.
#[tokio::test]
async fn a_stale_worker_session_cannot_assign() {
    use crate::app::mcp::{
        authenticated_mcp_owner, handle_coding_mcp_frame, AddressedSession, ControlPlane, Turn,
    };
    let mut project = WorkerProject::new();
    let worker = project.worker("Lead");
    let target = project.worker("Target");
    let (owner, project_id) = (project.owner.clone(), project.project_id.clone());
    let task = file(&mut project, "Late hand-off");
    let state = &mut project.state;
    state
        .session_registry
        .test_install_token(worker.clone(), "old-worker-token".into());
    let frame = json!({
        "task_id": worker, "session_token": "old-worker-token",
        "request": BridgeAction::TrackerAssignTask {
            task_id: task.clone(),
            assignee: json!({ "kind": "agent", "agent_id": target }),
            note: None,
            track: None,
            notify_user: None,
        },
    });
    let authenticated = authenticated_mcp_owner(&frame, &state.session_registry).unwrap();
    let Some(AddressedSession::Coding(generation)) = state.addressed_session(authenticated.into())
    else {
        panic!("the worker is addressed as a conversation session");
    };
    let thread_id = state
        .agent_conversation(&owner, Some(&worker))
        .unwrap()
        .id
        .clone();
    let reset = state.handle(req(
        "conversation.reset",
        json!({
            "project_id": project_id, "entity_id": owner, "agent_id": worker,
            "conversation_id": worker, "expected_thread_id": thread_id,
        }),
    ));
    assert_eq!(reset["ok"], true, "{reset:?}");

    let WorkerProject {
        _home, _tmp, state, ..
    } = project;
    let shared = Arc::new(Mutex::new(state));
    let clock = Arc::clone(&shared.lock().unwrap().frame_clock);
    let plane = ControlPlane::new(Arc::clone(&clock));
    let mut turn = Turn::take(&plane).await;
    let timer = clock.frame(crate::app::mcp::MCP_CONTROL_METHOD);
    let answered = handle_coding_mcp_frame(&shared, &frame, &generation, &timer, &mut turn).await;
    assert_eq!(answered.unwrap()["ok"], false);
    let mut app = shared.lock().unwrap();
    assert!(dispatches(&app, &task).is_empty());
    assert_eq!(notices(&mut app, &owner, &target), 0);
}

/// Removing a worker that holds a task keeps the task's record — who it was
/// handed to and the dispatch that did it — and says the worker is gone;
/// the turn still queued for the worker goes with it.
#[test]
fn removing_a_worker_keeps_its_tasks_record_and_drops_its_queued_turn() {
    let mut project = WorkerProject::new();
    let worker = project.worker("Holder");
    let owner = project.owner.clone();
    let task = file(&mut project, "Held work");
    let assigned = assign(
        &mut project.state,
        json!({ "task_id": task, "assignee": { "kind": "agent", "agent_id": worker } }),
    );
    let queued_for = |state: &AppState| {
        state
            .delivery_queue
            .queued()
            .any(|turn| turn.agent_id == worker)
    };
    assert!(queued_for(&project.state), "the hand-off is queued");

    let removed = project.state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": worker }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    assert!(!queued_for(&project.state), "its queued turn went with it");
    let read = project
        .state
        .handle(req("tasks.get", json!({ "task_id": task })));
    let stored = &read["result"]["task"];
    assert_eq!(stored["assignee"], assigned["task"]["assignee"]);
    assert_eq!(
        stored["identities"][&worker]["available"], false,
        "{stored}"
    );
    assert_eq!(stored["identities"][&worker]["name"], "Holder");
    assert_eq!(
        dispatches(&project.state, &task),
        vec![assigned["dispatch"]["operation_id"].as_str().unwrap()]
    );
}
