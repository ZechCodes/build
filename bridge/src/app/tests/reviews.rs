//! Review RPCs share the same service and authorization as agent tools.

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;

fn review_call(state: &mut AppState, method: &str, params: Value) -> Value {
    let answer = state.handle(req(method, params));
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

#[test]
fn review_snapshot_rpc_keeps_task_position_and_completion_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "review");
    let task = filed(&mut state, &project, "Review this workspace");
    let task_id = task["id"].as_str().unwrap();
    let before = review_call(&mut state, "tasks.review.get", json!({"task_id": task_id}));
    assert!(before["review"].is_null());
    let saved = review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task_id, "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    assert_eq!(saved["review"]["version"], 1);
    assert_eq!(saved["review"]["snapshots"][0]["number"], 1);
    assert_eq!(saved["review"]["snapshots"][0]["author"]["kind"], "user");
    let held = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert_eq!(held["task"]["status"], "backlog");
    let stale = state.handle(req(
        "tasks.review.complete",
        json!({
            "task_id": task_id, "expected_version": 0, "description": "Checked and pushed",
        }),
    ));
    assert_eq!(stale["error_code"], "stale_version", "{stale}");
    let completed = review_call(
        &mut state,
        "tasks.review.complete",
        json!({
            "task_id": task_id, "expected_version": 1, "description": "Checked and pushed",
        }),
    );
    assert_eq!(completed["review"]["state"], "completed");
    assert_eq!(completed["review"]["version"], 2);
    let held = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert_eq!(held["task"]["status"], "done");
    assert_eq!(held["task"]["state"], "open");
    let timeline = held["timeline"].as_array().unwrap();
    assert_eq!(timeline[timeline.len() - 2]["kind"], "moved");
    assert_eq!(
        timeline[timeline.len() - 2]["payload"],
        json!({"from": "backlog", "to": "done"})
    );
    assert_eq!(timeline[timeline.len() - 1]["kind"], "review_completed");
    assert!(state.workspaces.get(&workspace_id).is_some());
}

#[test]
fn review_comment_metadata_round_trips_and_rejects_foreign_context() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "comments");
    let task = filed(&mut state, &project, "Comment on review");
    let task_id = task["id"].as_str().unwrap();
    let saved = review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task_id, "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    let anchor = json!({
        "snapshot_id": snapshot["id"], "directory_id": snapshot["directories"][0]["id"],
        "path": "README.md", "side": "new", "line": 1,
    });
    let first = review_call(
        &mut state,
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Check this line", "anchor": anchor,
            "opinion": {"snapshot_id": snapshot["id"], "verdict": "request_changes"},
        }),
    );
    assert_eq!(first["comment"]["anchor"], anchor);
    let persisted = state
        .tracker_store()
        .unwrap()
        .load_tracker_comment(first["comment"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.anchor.unwrap().path, "README.md");
    let second = review_call(
        &mut state,
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Agreed", "reply_to": first["comment"]["id"],
        }),
    );
    assert_eq!(second["comment"]["reply_to"], first["comment"]["id"]);
    let invalid = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong", "anchor": {"snapshot_id": snapshot["id"],
            "directory_id": "other", "path": "README.md", "side": "new", "line": 1},
        }),
    ));
    assert_eq!(invalid["ok"], false, "{invalid}");
    let invalid_path = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task_id, "body": "Wrong path", "anchor": {
                "snapshot_id": snapshot["id"], "directory_id": snapshot["directories"][0]["id"],
                "path": "../README.md", "side": "new", "line": 1,
            },
        }),
    ));
    assert_eq!(invalid_path["ok"], false, "{invalid_path}");
    let foreign_task = filed(&mut state, &project, "Another task");
    let foreign_reply = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": foreign_task["id"], "body": "Wrong task",
            "reply_to": first["comment"]["id"],
        }),
    ));
    assert_eq!(foreign_reply["ok"], false, "{foreign_reply}");
    let replacement_workspace = workspace(&mut state, &project, "replacement");
    review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task_id, "workspace_id": replacement_workspace, "expected_version": 1,
        }),
    );
    let timeline = review_call(&mut state, "tasks.get", json!({"task_id": task_id}));
    assert!(timeline["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| {
            entry["id"] == first["comment"]["id"]
                && entry["opinion"]["verdict"] == "request_changes"
                && entry["anchor"] == anchor
        }));
}

#[test]
fn mcp_review_comment_uses_the_same_metadata_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "agent comments");
    let task = filed(&mut state, &project, "Ask for review");
    let (owner, agent) = project_agent(&mut state, &project);
    let saved = review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    let frame = json!({"jsonrpc":"2.0", "id": 1, "method":"tools/call",
    "params": {"name":"comment_task", "arguments": {
        "task_id": task["id"], "body": "Approved",
        "opinion": {"snapshot_id": snapshot["id"], "verdict":"approve"}
    }}});
    let action = crate::mcp::DoneServer::new(&agent)
        .handle_message(&frame.to_string())
        .action
        .unwrap();
    let result = state.agent_action(&owner, &agent, action).unwrap();
    assert_eq!(result["comment"]["opinion"]["verdict"], "approve");
    assert_eq!(result["comment"]["author"]["agent_id"], agent);
}

#[test]
fn review_rpc_reads_whole_saved_files_and_refuses_caller_supplied_locations() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "files");
    let task = filed(&mut state, &project, "Read unchanged files");
    let saved = review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({
            "task_id": task["id"], "workspace_id": workspace_id, "expected_version": 0,
        }),
    );
    let snapshot = &saved["review"]["snapshots"][0];
    assert_eq!(snapshot["directories"][0]["status"], "git", "{saved}");
    let mut params = json!({
        "task_id": task["id"], "snapshot_id": snapshot["id"],
        "directory_id": snapshot["directories"][0]["id"],
        "mode": "blob", "path": "README.md",
    });
    let checkout = state.workspaces.get(&workspace_id).unwrap().directories[0]
        .path
        .clone();
    std::fs::write(checkout.join("README.md"), "unsaved content").unwrap();
    let file = review_call(&mut state, "tasks.review.diff", params.clone());
    assert_eq!(
        file["content_b64"],
        crate::encoding::b64encode(b"# project\n")
    );
    assert_eq!(file["editable"], false);
    params["repo_path"] = json!(tmp.path());
    let refusal = state.handle(req("tasks.review.diff", params));
    assert_eq!(refusal["error_code"], "invalid_params", "{refusal}");
}

fn agent_review(
    state: &mut AppState,
    owner: &str,
    agent: &str,
    action: Value,
) -> Result<Value, String> {
    let action = serde_json::from_value(action).expect("review MCP action parses");
    state.agent_action(owner, agent, action)
}

#[test]
fn review_mcp_authenticates_actor_and_fences_both_task_and_workspace_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = tracked(tmp.path());
    let workspace_id = workspace(&mut state, &project, "tools");
    let task = filed(&mut state, &project, "Any agent may review");
    let (owner, agent) = project_agent(&mut state, &project);
    let saved = agent_review(
        &mut state,
        &owner,
        &agent,
        json!({
            "action": "tracker_snapshot_review", "base_overrides": {}, "task_id": task["id"],
            "workspace_id": workspace_id, "expected_version": 0,
        }),
    )
    .unwrap();
    assert_eq!(saved["review"]["snapshots"][0]["author"]["agent_id"], agent);
    let (_other_home, other_repo) = init_repo();
    let other_project = added_project(&mut state, &other_repo);
    let other_task = filed(&mut state, &other_project, "Another project's task");
    let refused = agent_review(
        &mut state,
        &owner,
        &agent,
        json!({
            "action": "tracker_get_review", "task_id": other_task["id"],
        }),
    )
    .unwrap_err();
    assert!(refused.starts_with("unknown task_id"), "{refused}");
    let other_workspace = workspace(&mut state, &other_project, "foreign");
    let refused = agent_review(
        &mut state,
        &owner,
        &agent,
        json!({
            "action": "tracker_snapshot_review", "base_overrides": {}, "task_id": task["id"],
            "workspace_id": other_workspace, "expected_version": 1,
        }),
    )
    .unwrap_err();
    assert!(refused.starts_with("unknown workspace_id"), "{refused}");
    let completed = agent_review(
        &mut state,
        &owner,
        &agent,
        json!({
            "action": "tracker_complete_review", "task_id": task["id"],
            "expected_version": 1, "description": "Reviewed and merged using my tools",
        }),
    )
    .unwrap();
    assert_eq!(
        completed["review"]["completion"]["actor"]["agent_id"],
        agent
    );
    let held = review_call(&mut state, "tasks.get", json!({"task_id": task["id"]}));
    let timeline = held["timeline"].as_array().unwrap();
    assert_eq!(timeline[timeline.len() - 2]["kind"], "moved");
    assert_eq!(timeline[timeline.len() - 2]["actor"]["agent_id"], agent);
    assert_eq!(timeline[timeline.len() - 1]["kind"], "review_completed");
    let conversation = state.handle(req(
        "thread.page",
        json!({"entity_id": owner, "agent_id": agent, "limit": 50}),
    ));
    assert_eq!(conversation["ok"], true, "{conversation}");
    let actions: Vec<_> = conversation["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["data"]["task_action"].as_object())
        .collect();
    assert_eq!(actions.len(), 1, "{conversation}");
    assert_eq!(actions[0]["action"], "moved");
    assert_eq!(actions[0]["to"], "done");
}
