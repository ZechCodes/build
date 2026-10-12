//! The coding tools a worker reads and staffs its project with, and the
//! project-administration tools it is refused.

use super::*;
use crate::mcp::{BridgeAction, DoneServer, McpSurface};

fn add_project_agent(name: &str) -> BridgeAction {
    BridgeAction::AddProjectAgent {
        name: Some(name.into()),
        notify_user: None,
        harness: None,
        model: None,
        effort: None,
        role: None,
        capability: None,
    }
}

/// A worker reads its project's whole roster — the pin first, each agent's
/// purpose, the pin by id — and adds another worker to it, unwatched and
/// stamped with who made it. An agent in one of the project's workspaces
/// reaches the same roster; neither can name another project.
#[test]
fn project_workers_list_and_add_project_agents_with_coding_tools() {
    let mut project = WorkerProject::new();
    let worker = project.worker("Rail scroll");
    let (owner, pin) = (project.owner.clone(), project.pin.clone());

    let listed = project
        .state
        .on_agent_mcp_action(&owner, &worker, BridgeAction::ListProjectAgents)
        .unwrap();
    assert_eq!(listed["project_id"], json!(project.project_id));
    assert_eq!(listed["entity_id"], json!(owner));
    assert_eq!(listed["pinned_agent_id"], json!(pin));
    assert_eq!(listed_ids(&listed), vec![pin.clone(), worker.clone()]);
    assert_eq!(listed["agents"][1]["project_role"], "worker");

    let added = project
        .state
        .on_agent_mcp_action(&owner, &worker, add_project_agent("Docs"))
        .unwrap();
    let docs = added["agent"]["id"].as_str().unwrap().to_string();
    assert_eq!(added["project_id"], json!(project.project_id));
    assert_eq!(added["entity_id"], json!(owner));
    assert_eq!(added["agent"]["project_role"], "worker");
    assert_eq!(added["agent"]["created_by"], json!(worker));
    assert_eq!(added["agent"]["watched"], false, "unwatched by default");
    assert_eq!(
        project.roster(),
        vec![pin.clone(), worker.clone(), docs.clone()]
    );
    assert_eq!(project.state.project_pin(&owner), Some(pin.as_str()));

    // An agent in one of the project's workspaces is on the coding surface
    // too, and the project it reads and staffs is its own.
    let (_, workspace_owner, coder) = project.workspace_agent("elsewhere");
    let from_workspace = project
        .state
        .on_agent_mcp_action(&workspace_owner, &coder, BridgeAction::ListProjectAgents)
        .unwrap();
    assert_eq!(from_workspace["entity_id"], json!(owner));
    assert_eq!(
        listed_ids(&from_workspace),
        vec![pin.clone(), worker.clone(), docs.clone()]
    );
    let delegated = project
        .state
        .on_agent_mcp_action(&workspace_owner, &coder, add_project_agent("Bench"))
        .unwrap();
    assert_eq!(delegated["entity_id"], json!(owner));
    assert_eq!(delegated["agent"]["created_by"], json!(coder));
    assert_eq!(project.roster().len(), 4);

    // Another project's worker sees its own roster, never this one.
    let (_other_home, other_repo) = a_repo();
    let other_id = added_project(&mut project.state, &other_repo);
    let other_owner = ensure(&mut project.state, &other_id);
    let other_worker = add_worker(&mut project.state, &other_owner, json!({}));
    let other = project
        .state
        .on_agent_mcp_action(&other_owner, &other_worker, BridgeAction::ListProjectAgents)
        .unwrap();
    assert_eq!(other["project_id"], json!(other_id));
    let other_ids = listed_ids(&other);
    assert!(other_ids.contains(&other_worker), "{other_ids:?}");
    assert!(!other_ids.iter().any(|id| project.roster().contains(id)));
}

/// A project nobody has talked to has no owner yet; reading its roster from
/// a workspace says so rather than minting one, and adding an agent mints
/// the owner — with its pin — first.
#[test]
fn adding_a_project_agent_from_a_workspace_mints_the_owner_with_its_pin() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "first");
    let workspace_owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let coder = state.handle(req("agent.add", json!({ "entity_id": workspace_owner })))["result"]
        ["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let empty = state
        .on_agent_mcp_action(&workspace_owner, &coder, BridgeAction::ListProjectAgents)
        .unwrap();
    assert_eq!(empty["entity_id"], Value::Null);
    assert_eq!(empty["agents"], json!([]));
    assert_eq!(state.project_conversation_run(&project_id), None);

    let added = state
        .on_agent_mcp_action(&workspace_owner, &coder, add_project_agent("Docs"))
        .unwrap();
    let owner = added["entity_id"].as_str().unwrap().to_string();
    let pin = state.project_pin(&owner).unwrap().to_string();
    let roster: Vec<_> = state
        .entity_agents(&owner)
        .unwrap()
        .iter()
        .map(|agent| agent.id.clone())
        .collect();
    assert_eq!(
        roster,
        vec![pin, added["agent"]["id"].as_str().unwrap().into()]
    );
}

/// What a worker may not do through MCP: administer the project's sources,
/// reclaim or compact for the project, or take the pin away by any tool it
/// does hold. A forged purpose on the tool call makes another worker.
#[test]
fn a_worker_cannot_administer_sources_or_retire_the_pin_through_mcp() {
    let mut project = WorkerProject::new();
    let worker = project.worker("Rail scroll");
    let (owner, pin) = (project.owner.clone(), project.pin.clone());
    let (workspace_id, _, _) = project.workspace_agent("elsewhere");
    assert_eq!(
        project.state.agent_mcp_surface(&owner, &worker),
        McpSurface::Coding
    );

    for administration in [
        BridgeAction::AddProjectSource {
            path: Some(project.repo.display().to_string()),
            remote: None,
            name: None,
            base_branch: None,
        },
        BridgeAction::RemoveProjectSource {
            source_id: "source-1".into(),
        },
        BridgeAction::ReclaimWorkspace {
            workspace_id: workspace_id.clone(),
        },
        BridgeAction::CompactAgent {
            agent_id: pin.clone(),
            instructions: "forget the plan".into(),
        },
    ] {
        let tool = administration.tool_name();
        let refused = project
            .state
            .on_agent_mcp_action(&owner, &worker, administration)
            .unwrap_err();
        assert!(refused.contains("coding surface"), "{tool}: {refused}");
    }

    let removal = project.state.on_agent_mcp_action(
        &owner,
        &worker,
        BridgeAction::RemoveWorkspaceAgent {
            workspace_id: workspace_id.clone(),
            agent_id: pin.clone(),
        },
    );
    assert!(removal.is_err(), "{removal:?}");

    // The tool call a launched worker's server parses: whatever purpose the
    // arguments claim, none of them reaches agent.add.
    let server = DoneServer::launched(&worker, Some("coding"));
    let parsed = server.handle_message(
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "add_project_agent", "arguments": {
                "name": "Forged", "project_role": "coordinator", "role": "planner"
            }}
        })
        .to_string(),
    );
    let action = parsed.action.expect("the call parses");
    let added = project
        .state
        .on_agent_mcp_action(&owner, &worker, action)
        .unwrap();
    assert_eq!(added["agent"]["project_role"], "worker");
    assert_eq!(project.state.project_pin(&owner), Some(pin.as_str()));
    let coordinators = super::super::project_coordinator::coordinators(&project.state, &owner);
    assert_eq!(coordinators, vec![pin]);
}

/// Both working surfaces list the two project-agent tools; the router does
/// not, and the source tools stay the pin's.
#[test]
fn the_project_agent_tools_are_on_both_working_surfaces() {
    let coding = DoneServer::tool_names_of(McpSurface::Coding);
    let pinned = DoneServer::tool_names_of(McpSurface::Project);
    let router = DoneServer::tool_names_of(McpSurface::Router);
    for tool in ["list_project_agents", "add_project_agent"] {
        assert!(coding.contains(&tool.to_string()), "coding lacks {tool}");
        assert!(pinned.contains(&tool.to_string()), "project lacks {tool}");
        assert!(!router.contains(&tool.to_string()), "router has {tool}");
    }
    assert!(!coding.contains(&"add_project_source".to_string()));
}
