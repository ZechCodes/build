//! Who made an agent (#216): the agent whose Build MCP call put it there.
//!
//! An agent's activity panel lists the Build agents it created beside its
//! harness sub-agents, and counts itself running while any of them runs. The
//! bridge records the one fact that needs — `created_by` on the agent, carried
//! on every digest — and the client reads the rest off its cached rows.

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::tracker_tools::coding_agent;
use super::*;
use crate::mcp::BridgeAction;

fn add_workspace_agent(workspace_id: &str) -> BridgeAction {
    BridgeAction::AddWorkspaceAgent {
        workspace_id: workspace_id.to_string(),
        notify_user: None,
        harness: None,
        model: None,
        effort: None,
        name: None,
        role: None,
        capability: None,
    }
}

fn digest_on_row(state: &mut AppState, entity_id: &str, agent_id: &str) -> Value {
    let detail = run_detail(state, json!({ "run_id": entity_id }));
    assert_eq!(detail["ok"], true, "{detail:?}");
    detail["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == agent_id)
        .cloned()
        .expect("the row carries the agent")
}

#[test]
fn an_agent_added_over_mcp_names_the_agent_that_added_it() {
    let (_home, repo) = crate::git_fixture::init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = super::project_agent::rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "workers");
    let (owner, caller) = project_agent(&mut state, &project_id);

    let added = state
        .agent_action(&owner, &caller, add_workspace_agent(&workspace_id))
        .expect("a project agent puts an agent on its workspace");
    let entity_id = added["entity_id"].as_str().unwrap().to_string();
    let worker = added["agent"]["id"].as_str().unwrap().to_string();

    assert_eq!(added["agent"]["created_by"], caller, "{added:?}");
    assert_eq!(
        state.runs[&entity_id].agents.by_id(&worker).unwrap().created_by.as_deref(),
        Some(caller.as_str())
    );
    assert_eq!(digest_on_row(&mut state, &entity_id, &worker)["created_by"], caller);
}

#[test]
fn an_agent_the_user_added_names_no_creator() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = coding_agent(&mut state, &project_id, "mine");

    assert_eq!(digest_on_row(&mut state, &entity_id, &agent_id)["created_by"], Value::Null);
}

/// The wire cannot claim a creator: only an MCP call made by an agent can.
#[test]
fn agent_add_over_rpc_refuses_a_creator() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = coding_agent(&mut state, &project_id, "mine");

    let refused = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "created_by": agent_id }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
}

#[test]
fn a_task_assigned_to_a_new_agent_names_the_agent_that_assigned_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let workspace_id = workspace(&mut state, &project_id, "workers");
    let task_id = filed(&mut state, &project_id, "new worker")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assigned = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            BridgeAction::TrackerAssignTask {
                task_id,
                assignee: json!({ "kind": "new_agent", "workspace_id": workspace_id }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("the MCP call assigns to a new agent");
    let entity_id = assigned["dispatch"]["entity_id"].as_str().unwrap().to_string();
    let agent_id = assigned["dispatch"]["agent_id"].as_str().unwrap().to_string();

    assert_eq!(digest_on_row(&mut state, &entity_id, &agent_id)["created_by"], caller.1);
}

#[test]
fn a_task_assigned_to_a_new_workspace_names_the_agent_that_assigned_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let task_id = filed(&mut state, &project_id, "new workspace worker")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assigned = state
        .agent_action(
            &caller.0,
            &caller.1,
            BridgeAction::TrackerAssignTask {
                task_id,
                assignee: json!({ "kind": "new_workspace", "isolation": "worktree" }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("the MCP call cuts a workspace and assigns its agent");
    let entity_id = assigned["dispatch"]["entity_id"].as_str().unwrap().to_string();
    let agent_id = assigned["dispatch"]["agent_id"].as_str().unwrap().to_string();

    assert_eq!(digest_on_row(&mut state, &entity_id, &agent_id)["created_by"], caller.1);
}

/// A task the USER hands to a new agent was not made by any agent.
#[test]
fn a_task_the_user_assigns_to_a_new_agent_names_no_creator() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "workers");
    let task_id = filed(&mut state, &project_id, "by hand")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assigned = state.handle(req(
        "tasks.assign",
        json!({
            "task_id": task_id,
            "assignee": { "kind": "new_agent", "workspace_id": workspace_id },
        }),
    ));
    assert_eq!(assigned["ok"], true, "{assigned:?}");
    let dispatch = &assigned["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();

    assert_eq!(digest_on_row(&mut state, &entity_id, &agent_id)["created_by"], Value::Null);
}
