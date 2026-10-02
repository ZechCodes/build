//! A move to Done and explicit completion share one atomic review writer.

use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::BridgeAction;
use crate::reviews::model::ReviewSnapshot;
use crate::tracker::Actor;

fn save_review(state: &AppState, task_id: &str, workspace_id: &str) {
    state
        .tracker_store()
        .unwrap()
        .save_review_snapshot(
            task_id,
            workspace_id,
            0,
            ReviewSnapshot {
                id: format!("snapshot-{task_id}"),
                number: 0,
                created_at: crate::store::now_rfc3339(),
                author: Actor::User,
                directories: Vec::new(),
            },
        )
        .unwrap();
}

fn read_task(state: &mut AppState, task_id: &str) -> Value {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"].clone()
}

#[test]
fn moving_review_tasks_to_done_completes_them_for_user_and_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "review");
    let task_id = filed(&mut state, &project_id, "user review")["id"]
        .as_str()
        .unwrap()
        .to_string();
    save_review(&state, &task_id, &workspace_id);

    let rpc = state.handle(req(
        "tasks.update",
        json!({ "task_id": task_id, "status": "done", "body": "also saved" }),
    ));
    assert_eq!(rpc["ok"], true, "{rpc}");
    assert_eq!(rpc["result"]["task"]["body"], "also saved");
    assert_eq!(rpc["result"]["task"]["status"], "done");
    let review = state
        .tracker_store()
        .unwrap()
        .load_review(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(review.version, 2);
    assert_eq!(
        review.state,
        crate::reviews::records::ReviewState::Completed
    );
    assert_eq!(
        review.completion.as_ref().unwrap().description,
        "Marked done"
    );
    assert_eq!(review.completion.as_ref().unwrap().actor, Actor::User);
    let user_read = read_task(&mut state, &task_id);
    assert_eq!(user_read["task"]["state"], "open");
    assert!(user_read["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["kind"] == "review_completed" && entry["payload"]["description"] == "Marked done"
        }));

    let agent = coding_agent(&mut state, &project_id, "agent");
    let agent_task = filed(&mut state, &project_id, "agent review")["id"]
        .as_str()
        .unwrap()
        .to_string();
    save_review(&state, &agent_task, &workspace_id);
    let mcp = call(
        &mut state,
        &agent,
        BridgeAction::TrackerMoveTask {
            task_id: agent_task.clone(),
            status: "done".into(),
            track: None,
        },
    )
    .unwrap();
    assert_eq!(mcp["task"]["status"], "done");
    let review = state
        .tracker_store()
        .unwrap()
        .load_review(&agent_task)
        .unwrap()
        .unwrap();
    assert_eq!(review.version, 2);
    assert_eq!(
        review.completion.as_ref().unwrap().description,
        "Marked done"
    );
    assert_eq!(
        review.completion.as_ref().unwrap().actor,
        Actor::Agent { agent_id: agent.1 }
    );
}

#[test]
fn review_tasks_allow_other_moves_closure_and_edits_after_completion() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "review");
    let task_id = filed(&mut state, &project_id, "needs review")["id"]
        .as_str()
        .unwrap()
        .to_string();
    save_review(&state, &task_id, &workspace_id);

    let moved = state.handle(req(
        "tasks.update",
        json!({ "task_id": task_id, "status": "in_review" }),
    ));
    assert_eq!(moved["ok"], true, "{moved}");
    let closed = state.handle(req("tasks.close", json!({ "task_id": task_id })));
    assert_eq!(closed["ok"], true, "{closed}");
    let reopened = state.handle(req("tasks.reopen", json!({ "task_id": task_id })));
    assert_eq!(reopened["ok"], true, "{reopened}");

    let completed = state.handle(req(
        "tasks.review.complete",
        json!({
            "task_id": task_id,
            "expected_version": 1,
            "description": "merged and checked with my tools",
        }),
    ));
    assert_eq!(completed["ok"], true, "{completed}");
    let edited = state.handle(req(
        "tasks.update",
        json!({ "task_id": task_id, "status": "done", "body": "follow-up note" }),
    ));
    assert_eq!(edited["ok"], true, "{edited}");
    assert_eq!(edited["result"]["task"]["body"], "follow-up note");
    assert_eq!(edited["result"]["task"]["status"], "done");
    let comment = state.handle(req(
        "tasks.comment",
        json!({ "task_id": task_id, "body": "thanks" }),
    ));
    assert_eq!(comment["ok"], true, "{comment}");
}

#[test]
fn failed_done_move_leaves_review_task_and_timeline_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "review");
    let task_id = filed(&mut state, &project_id, "atomic review")["id"]
        .as_str()
        .unwrap()
        .to_string();
    save_review(&state, &task_id, &workspace_id);
    let before = read_task(&mut state, &task_id);
    state.tracker_store().unwrap().fail_next_write();
    let failed = state.handle(req(
        "tasks.update",
        json!({ "task_id": task_id, "status": "done", "body": "do not save" }),
    ));
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(read_task(&mut state, &task_id), before);
    let review = state
        .tracker_store()
        .unwrap()
        .load_review(&task_id)
        .unwrap()
        .unwrap();
    assert_eq!(review.version, 1);
    assert_eq!(review.state, crate::reviews::records::ReviewState::Open);
}

#[test]
fn ordinary_tasks_still_move_to_done_from_rpc_and_mcp() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let rpc_task = filed(&mut state, &project_id, "ordinary rpc")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let rpc = state.handle(req(
        "tasks.update",
        json!({ "task_id": rpc_task, "status": "done" }),
    ));
    assert_eq!(rpc["ok"], true, "{rpc}");

    let agent = coding_agent(&mut state, &project_id, "agent");
    let mcp_task = filed(&mut state, &project_id, "ordinary mcp")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mcp = call(
        &mut state,
        &agent,
        BridgeAction::TrackerMoveTask {
            task_id: mcp_task.clone(),
            status: "done".into(),
            track: None,
        },
    )
    .unwrap();
    assert_eq!(mcp["task"]["status"], "done");
}
