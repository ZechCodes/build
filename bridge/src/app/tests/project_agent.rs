//! The project agent's surface: what it may read, and what it is refused.
//!
//! The scope is the project its owner is bound to. Nothing a tool call carries
//! can widen it, and nothing another surface has is reachable from it.

use super::*;
use crate::mcp::BridgeAction;

fn context(state_root: &Path) -> HarnessContext {
    HarnessContext::resolved(state_root.join("mcp.sock"), state_root.to_path_buf()).unwrap()
}

fn rooted(state_root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        state_root.join("worktrees"),
        "main",
        true,
        context(state_root),
    )
}

fn added_project(state: &mut AppState, repo: &Path) -> String {
    let project = state.handle(req("project.add", json!({ "path": repo })));
    assert_eq!(project["ok"], true, "{project:?}");
    project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A project's conversation owner and the project agent on it.
fn project_agent(state: &mut AppState, project_id: &str) -> (String, String) {
    let ensured = state.handle(req(
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": owner })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    assert!(crate::agent::is_project_agent(&agent_id), "{agent_id}");
    (owner, agent_id)
}

fn workspace(state: &mut AppState, project_id: &str, name: &str) -> String {
    let created = state.handle(req(
        "workspace.create",
        json!({ "project_id": project_id, "name": name, "isolation": "worktree" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The two reads answer what the client verbs answer, for the project the
/// agent's owner is bound to — no project id is passed, because there is
/// nowhere for one to come from.
#[test]
fn a_project_agent_reads_its_own_workspaces_and_the_agents_on_them() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let first = workspace(&mut state, &project_id, "one");
    let second = workspace(&mut state, &project_id, "two");
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let listed = state
        .on_agent_mcp_action(&owner, &agent_id, BridgeAction::ListWorkspaces)
        .expect("a project agent reads its own workspaces");
    let ids: Vec<&str> = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&first.as_str()) && ids.contains(&second.as_str()),
        "{listed:?}"
    );
    let client = state.handle(req("workspace.list", json!({ "project_id": project_id })));
    assert_eq!(listed["workspaces"], client["result"]["workspaces"]);

    // A workspace nobody has talked to has no conversation owner, so no agents.
    let empty = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: first.clone(),
            },
        )
        .expect("a workspace with no owner still answers");
    assert_eq!(empty["agents"], json!([]), "{empty:?}");

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": first }),
    ));
    let workspace_owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": workspace_owner })));
    let workspace_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let agents = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: first.clone(),
            },
        )
        .expect("a project agent reads the agents on its workspaces");
    assert_eq!(agents["workspace_id"], first);
    assert_eq!(agents["entity_id"], workspace_owner);
    assert_eq!(agents["agents"][0]["id"], workspace_agent, "{agents:?}");
}

/// The scope is the owner's binding, so a workspace of another project is
/// refused rather than read — whatever the tool call says.
#[test]
fn a_project_agent_is_refused_a_workspace_outside_its_project() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let mine = added_project(&mut state, &repo);
    let theirs = added_project(&mut state, &other_repo);
    let elsewhere = workspace(&mut state, &theirs, "theirs");
    let (owner, agent_id) = project_agent(&mut state, &mine);

    let refused = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: elsewhere.clone(),
            },
        )
        .unwrap_err();
    assert!(
        refused.contains(&elsewhere) && refused.contains(&mine),
        "{refused}"
    );

    let unknown = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: "ws-nope".to_string(),
            },
        )
        .unwrap_err();
    assert!(
        unknown.contains("unknown workspace_id: ws-nope"),
        "{unknown}"
    );

    // The other project's own agent reads it, which is what makes the refusal
    // about scope rather than about the workspace.
    let (their_owner, their_agent) = project_agent(&mut state, &theirs);
    let listed = state
        .on_agent_mcp_action(
            &their_owner,
            &their_agent,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: elsewhere.clone(),
            },
        )
        .expect("the workspace's own project reads it");
    assert_eq!(listed["workspace_id"], elsewhere);
}

/// The socket enforces the surface on the frames themselves: a project agent
/// cannot write its way onto the router's surface, and a coding agent cannot
/// write its way onto the project's.
#[test]
fn a_project_session_reaches_no_other_surfaces_tools() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "one");
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let reaching_out = state
        .on_agent_mcp_action(&owner, &agent_id, BridgeAction::ListProjects)
        .unwrap_err();
    assert!(
        reaching_out.contains("list_projects")
            && reaching_out.contains("router tool")
            && reaching_out.contains("project surface"),
        "{reaching_out}"
    );

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let workspace_owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": workspace_owner })));
    let coding_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let reaching_in = state
        .on_agent_mcp_action(
            &workspace_owner,
            &coding_agent,
            BridgeAction::ListWorkspaces,
        )
        .unwrap_err();
    assert!(
        reaching_in.contains("list_workspaces")
            && reaching_in.contains("project tool")
            && reaching_in.contains("coding surface"),
        "{reaching_in}"
    );

    // The conversation tools are on both surfaces, so the project agent keeps
    // its own: naming its topic is not another surface's verb.
    let topic = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::SetTopic {
                topic: "Cut a workspace".to_string(),
            },
        )
        .expect("a project agent names its conversation");
    assert_eq!(topic["topic"], "Cut a workspace", "{topic:?}");
}
