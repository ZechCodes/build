//! The project agent's surface: what it may read, and what it is refused.
//!
//! The scope is the project its owner is bound to. Nothing a tool call carries
//! can widen it, and nothing another surface has is reachable from it.

use super::*;
use crate::mcp::BridgeAction;

pub(super) fn context(state_root: &Path) -> HarnessContext {
    HarnessContext::resolved(state_root.join("mcp.sock"), state_root.to_path_buf()).unwrap()
}

pub(super) fn rooted(state_root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        state_root.join("worktrees"),
        "main",
        true,
        context(state_root),
    )
}

pub(super) fn added_project(state: &mut AppState, repo: &Path) -> String {
    let project = state.handle(req("project.add", json!({ "path": repo })));
    assert_eq!(project["ok"], true, "{project:?}");
    project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A project's conversation owner and the project agent on it.
pub(super) fn project_agent(state: &mut AppState, project_id: &str) -> (String, String) {
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

pub(super) fn workspace(state: &mut AppState, project_id: &str, name: &str) -> String {
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
/// write its way onto the two verbs that stay the project agent's.
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
            BridgeAction::AddProjectSource {
                path: Some("/tmp/elsewhere".to_string()),
                remote: None,
                name: None,
                base_branch: None,
            },
        )
        .unwrap_err();
    assert!(
        reaching_in.contains("add_project_source")
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

/// A project agent is told what it is, not how to build: the cold prompt it is
/// delivered with is the project's, and the coding protocol stays with the
/// agents that have a checkout to apply it to.
#[test]
fn a_project_agents_cold_prompt_is_its_own_and_not_a_coding_agents() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let name = state.projects.get(&project_id).unwrap().name.clone();
    let workspace_id = workspace(&mut state, &project_id, "one");
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let cold =
        crate::orchestrator::conversation_prompt(crate::orchestrator::NEW_THREAD_MESSAGES_PROMPT);
    let prompt = state.cold_prompt_with_catch_up(&owner, &agent_id, &cold);
    assert!(
        prompt.contains(&format!("agent for the project {name}")),
        "{prompt}"
    );
    assert!(prompt.contains("list_workspaces"), "{prompt}");
    assert!(
        !prompt.contains("Build conversation protocol"),
        "the coding protocol is about phases and a diff: {prompt}"
    );

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let workspace_owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": workspace_owner })));
    let coding_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let coding = state.cold_prompt_with_catch_up(&workspace_owner, &coding_agent, &cold);
    assert!(coding.contains("Build conversation protocol"), "{coding}");
    assert!(
        !coding.contains("agent for the project"),
        "the project agent's own prompt is not a coding agent's: {coding}"
    );
}

/// An agent working in a checkout is bound to a project too — the one its run
/// stands in — so the workspace tools are its as well, scoped by that binding
/// and by nothing the call says.
///
/// This is the whole of what commit 2 promised: it reads and cuts workspaces in
/// its own project, a workspace of another project is refused exactly as it is
/// for the project agent, and the two verbs that change what a project is made
/// of are not on its surface at all.
#[test]
fn a_coding_agent_works_the_workspaces_of_its_own_project_and_no_others() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let mine = added_project(&mut state, &repo);
    let theirs = added_project(&mut state, &other_repo);
    let standing_on = workspace(&mut state, &mine, "one");
    let elsewhere = workspace(&mut state, &theirs, "theirs");

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": standing_on }),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let coding_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    assert!(
        !crate::agent::is_project_agent(&coding_agent),
        "{coding_agent}"
    );
    assert_eq!(
        crate::mcp::McpSurface::for_owner(&coding_agent),
        crate::mcp::McpSurface::Coding
    );

    // It reads its own project's workspaces, naming no project — there is
    // nowhere in the call for one to come from.
    let listed = state
        .on_agent_mcp_action(&run_id, &coding_agent, BridgeAction::ListWorkspaces)
        .expect("a coding agent reads the workspaces of the project it stands in");
    let ids: Vec<&str> = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&standing_on.as_str()), "{listed:?}");
    assert!(
        !ids.contains(&elsewhere.as_str()),
        "the other project's workspace is not in its list: {listed:?}"
    );

    // And cuts one there, through the same verb the project agent's tool calls.
    let created = state
        .agent_action(
            &run_id,
            &coding_agent,
            BridgeAction::CreateWorkspace {
                name: "the other half".to_string(),
                isolation: Some("worktree".to_string()),
            },
        )
        .expect("a coding agent cuts a workspace in its own project");
    let cut = created["workspace_id"].as_str().unwrap().to_string();
    let ours = state.handle(req("workspace.list", json!({ "project_id": mine })));
    let names: Vec<&str> = ours["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"the other half"), "{ours:?}");
    let not_theirs = state.handle(req("workspace.list", json!({ "project_id": theirs })));
    let their_ids: Vec<&str> = not_theirs["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap())
        .collect();
    assert!(!their_ids.contains(&cut.as_str()), "{not_theirs:?}");

    // Every workspace argument is checked against that binding, read or write,
    // the same way and with the same words the project agent is refused with.
    for action in [
        BridgeAction::ListWorkspaceAgents {
            workspace_id: elsewhere.clone(),
        },
        BridgeAction::DeleteWorkspace {
            workspace_id: elsewhere.clone(),
        },
        BridgeAction::AddWorkspaceAgent {
            workspace_id: elsewhere.clone(),
            notify_user: None,
            harness: None,
            model: None,
            effort: None,
            name: None,
            role: None,
            capability: None,
        },
        BridgeAction::MessageWorkspaceAgent {
            workspace_id: elsewhere.clone(),
            agent_id: None,
            body: "start on the rail".to_string(),
        },
        BridgeAction::AddWorkspaceDirectory {
            workspace_id: elsewhere.clone(),
            source_id: None,
            path: Some("/tmp".to_string()),
            remote: None,
            name: None,
        },
    ] {
        let tool = action.tool_name();
        let refused = state
            .agent_action(&run_id, &coding_agent, action)
            .unwrap_err();
        assert!(
            refused.contains(&elsewhere) && refused.contains(&mine),
            "{tool}: {refused}"
        );
    }
    let still_theirs = state.handle(req("workspace.list", json!({ "project_id": theirs })));
    assert_eq!(
        still_theirs["result"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "nothing was done to the other project: {still_theirs:?}"
    );

    // What stays the project agent's: the folders the NEXT workspace is cut
    // from. Refused on the frame, and never advertised in the first place.
    let refused = state
        .on_agent_mcp_action(
            &run_id,
            &coding_agent,
            BridgeAction::AddProjectSource {
                path: Some(other_repo.display().to_string()),
                remote: None,
                name: None,
                base_branch: None,
            },
        )
        .unwrap_err();
    assert!(
        refused.contains("add_project_source")
            && refused.contains("project tool")
            && refused.contains("coding surface"),
        "{refused}"
    );

    let coding_tools = crate::mcp::DoneServer::tool_names_of(crate::mcp::McpSurface::Coding);
    for workspace_tool in [
        "list_workspaces",
        "list_workspace_agents",
        "create_workspace",
        "delete_workspace",
        "add_workspace_directory",
        "remove_workspace_directory",
        "add_workspace_agent",
        "remove_workspace_agent",
        "message_workspace_agent",
    ] {
        assert!(
            coding_tools.contains(&workspace_tool.to_string()),
            "{workspace_tool} missing from the coding surface: {coding_tools:?}"
        );
    }
    for project_only in ["add_project_source", "remove_project_source"] {
        assert!(
            !coding_tools.contains(&project_only.to_string()),
            "{project_only} is the project agent's alone: {coding_tools:?}"
        );
    }
}

/// Build will not take the ground out from under an agent.
///
/// `delete_workspace` is the same tool an agent uses on its siblings, and the
/// call says nothing about where the caller is standing — so the one workspace
/// it must never act on is the one the caller is in. Refused at the handler,
/// not left to the prompt: an agent that got it wrong would end its own session
/// and take its uncommitted work with it, and there is nothing left to tell
/// afterwards.
#[test]
fn a_coding_agent_cannot_delete_the_workspace_it_is_standing_in() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let standing_on = workspace(&mut state, &project_id, "mine");
    let sibling = workspace(&mut state, &project_id, "next door");

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": standing_on }),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let coding_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    let refused = state
        .agent_action(
            &run_id,
            &coding_agent,
            BridgeAction::DeleteWorkspace {
                workspace_id: standing_on.clone(),
            },
        )
        .expect_err("an agent cannot delete the workspace it is working in");
    assert_eq!(
        refused,
        "Build cannot remove the workspace this agent is working in."
    );
    assert!(
        state.workspaces.get(&standing_on).is_some(),
        "the workspace survived the refusal"
    );

    // A sibling in the same project is still its business.
    let deleted = state
        .agent_action(
            &run_id,
            &coding_agent,
            BridgeAction::DeleteWorkspace {
                workspace_id: sibling.clone(),
            },
        )
        .expect("a workspace it is not standing in is still deletable");
    assert_eq!(deleted["deleted"], true, "{deleted:?}");
    assert!(state.workspaces.get(&sibling).is_none());

    // And the project agent, which stands in no workspace at all, is unaffected
    // — including for the workspace the coding agent is working in.
    let (owner, project_agent_id) = project_agent(&mut state, &project_id);
    let by_the_project_agent = state
        .agent_action(
            &owner,
            &project_agent_id,
            BridgeAction::DeleteWorkspace {
                workspace_id: standing_on.clone(),
            },
        )
        .expect("the project agent stands nowhere and deletes any of them");
    assert_eq!(
        by_the_project_agent["deleted"], true,
        "{by_the_project_agent:?}"
    );
    assert!(state.workspaces.get(&standing_on).is_none());
}

/// The same refusal, one directory down: a workspace conversation's checkout IS
/// the workspace root, so every directory under it is ground that agent stands
/// on. A directory of a workspace it is not in stays removable.
#[test]
fn a_coding_agent_cannot_remove_the_directory_its_checkout_is_in() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let assets = state_root.join("assets");
    std::fs::create_dir(&assets).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let standing_on = workspace(&mut state, &project_id, "mine");
    let sibling = workspace(&mut state, &project_id, "next door");

    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": standing_on }),
    ));
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    let coding_agent = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    // One more directory in each, so there is something to take out of both.
    let directory_of = |state: &mut AppState, workspace_id: &str| -> String {
        let grown = state
            .agent_action(
                &run_id,
                &coding_agent,
                BridgeAction::AddWorkspaceDirectory {
                    workspace_id: workspace_id.to_string(),
                    source_id: None,
                    path: Some(assets.display().to_string()),
                    remote: None,
                    name: Some("assets".to_string()),
                },
            )
            .expect("a directory is added");
        grown["directories"].as_array().unwrap()[1]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let mine = directory_of(&mut state, &standing_on);
    let theirs = directory_of(&mut state, &sibling);

    let refused = state
        .agent_action(
            &run_id,
            &coding_agent,
            BridgeAction::RemoveWorkspaceDirectory {
                workspace_id: standing_on.clone(),
                directory_id: mine.clone(),
            },
        )
        .expect_err("an agent cannot remove a directory of the workspace it is in");
    assert_eq!(
        refused,
        "Build cannot remove the directory this agent is working in."
    );
    assert_eq!(
        state
            .workspaces
            .get(&standing_on)
            .unwrap()
            .directories
            .len(),
        2,
        "the directory survived the refusal"
    );

    let shrunk = state
        .agent_action(
            &run_id,
            &coding_agent,
            BridgeAction::RemoveWorkspaceDirectory {
                workspace_id: sibling.clone(),
                directory_id: theirs,
            },
        )
        .expect("a directory of a workspace it is not in is still removable");
    assert_eq!(
        shrunk["directories"].as_array().unwrap().len(),
        1,
        "{shrunk:?}"
    );

    // The project agent is standing nowhere, so it removes either.
    let (owner, project_agent_id) = project_agent(&mut state, &project_id);
    let by_the_project_agent = state
        .agent_action(
            &owner,
            &project_agent_id,
            BridgeAction::RemoveWorkspaceDirectory {
                workspace_id: standing_on.clone(),
                directory_id: mine,
            },
        )
        .expect("the project agent stands nowhere and removes any of them");
    assert_eq!(
        by_the_project_agent["directories"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{by_the_project_agent:?}"
    );
}

/// A project agent's message reaches its own conversation, which is what makes
/// the conversation tools generic rather than the coding surface's.
#[test]
fn a_project_agents_message_lands_on_its_own_conversation() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let posted = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::PostThreadMessage {
                still_working: false,
                body: "one workspace, nobody in it".to_string(),
                options: Vec::new(),
            },
        )
        .expect("a project agent speaks to the user");
    assert!(posted["message_id"].is_string(), "{posted:?}");

    let conversation = state.handle(req(
        "thread.page",
        json!({ "entity_id": owner, "agent_id": agent_id }),
    ));
    let bodies: Vec<&str> = conversation["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page of items: {conversation:?}"))
        .iter()
        .filter_map(|item| item["data"]["body"].as_str())
        .collect();
    assert!(
        bodies.contains(&"one workspace, nobody in it"),
        "{conversation:?}"
    );
}

/// The write tools are scoped the way the reads are: `create_workspace` cuts
/// into the owner's project, and the call carries no project for it to cut
/// into anywhere else.
#[test]
fn a_project_agent_cuts_a_workspace_in_the_project_it_belongs_to() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let mine = added_project(&mut state, &repo);
    let theirs = added_project(&mut state, &other_repo);
    let (owner, agent_id) = project_agent(&mut state, &mine);

    let created = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::CreateWorkspace {
                name: "read the router".to_string(),
                isolation: None,
            },
        )
        .expect("a project agent cuts a workspace in its own project");
    let workspace_id = created["workspace_id"].as_str().unwrap().to_string();
    assert_eq!(created["created_by_agent"], true, "{created:?}");

    let detail = state.handle(req(
        "workspace.get",
        json!({ "workspace_id": workspace_id }),
    ));
    assert_eq!(detail["result"]["created_by_agent"], true, "{detail:?}");

    let ours = state.handle(req("workspace.list", json!({ "project_id": mine })));
    let names: Vec<&str> = ours["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"read the router"), "{ours:?}");
    let workspace = ours["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["workspace_id"] == workspace_id)
        .unwrap();
    assert_eq!(workspace["created_by_agent"], true, "{workspace:?}");

    state.workspaces.reload().unwrap();
    assert!(
        state
            .workspaces
            .get(&workspace_id)
            .unwrap()
            .created_by_agent,
        "the manifest remembers that an agent created the empty workspace"
    );

    let elsewhere = state.handle(req("workspace.list", json!({ "project_id": theirs })));
    let ids: Vec<&str> = elsewhere["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&workspace_id.as_str()), "{elsewhere:?}");
}

/// Putting an agent on a workspace and taking it off again, through the verbs
/// the rail's own cog calls. The workspace's conversation owner is minted on
/// the way in, because a workspace nobody has talked to has none and there is
/// otherwise nowhere for the agent to live.
#[test]
fn a_project_agent_puts_an_agent_on_a_workspace_and_takes_it_off() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "one");
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let added = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: None,
                role: None,
                capability: None,
            },
        )
        .expect("a project agent puts an agent on its workspace");
    assert_eq!(added["workspace_id"], workspace_id);
    let entity_id = added["entity_id"].as_str().unwrap().to_string();
    let worker = added["agent"]["id"].as_str().unwrap().to_string();

    let listed = state
        .on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: workspace_id.clone(),
            },
        )
        .expect("the agent it just added is on the workspace");
    assert_eq!(listed["entity_id"], entity_id);
    assert_eq!(listed["agents"][0]["id"], worker, "{listed:?}");

    let removed = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::RemoveWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                agent_id: worker.clone(),
            },
        )
        .expect("a project agent takes an agent back off");
    assert_eq!(removed["workspace_id"], workspace_id);
    assert_eq!(removed["agent_id"], worker);
    assert_eq!(removed["agents"], json!([]), "{removed:?}");
}

/// MCP creation uses the same watch rule as agent.add, and the digest the
/// browser caches carries the resulting value for board and detail reads.
#[test]
fn a_project_agent_can_explicitly_watch_a_new_workspace_agent() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let quiet_workspace = workspace(&mut state, &project_id, "quiet");
    let followed_workspace = workspace(&mut state, &project_id, "followed");
    let (owner, caller) = project_agent(&mut state, &project_id);
    assert!(
        state.runs[&owner].agents.by_id(&caller).unwrap().watched,
        "agent.add from the UI watches the agent by default"
    );

    for (workspace_id, notify_user, watched) in [
        (&quiet_workspace, None, false),
        (&followed_workspace, Some(true), true),
    ] {
        let added = state
            .on_agent_mcp_action(
                &owner,
                &caller,
                BridgeAction::AddWorkspaceAgent {
                    workspace_id: workspace_id.clone(),
                    notify_user,
                    harness: None,
                    model: None,
                    effort: None,
                    name: None,
                    role: None,
                    capability: None,
                },
            )
            .expect("the MCP call creates an agent");
        assert_eq!(added["agent"]["watched"], watched, "{added:?}");
        let entity_id = added["entity_id"].as_str().unwrap();
        let detail = state.handle(req("run.get", json!({ "run_id": entity_id })));
        assert_eq!(
            detail["result"]["agents"][0]["watched"], watched,
            "{detail:?}"
        );
        let board = state.handle(req("board.list", json!({})));
        let run = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["run_id"] == entity_id)
            .expect("the board reports every run");
        assert_eq!(run["agents"][0]["watched"], watched, "{run:?}");
        let in_inbox = board["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["run_id"] == entity_id);
        assert_eq!(in_inbox, watched, "{board:?}");
    }
}

/// The write tools are scoped the way the reads are: a workspace of another
/// project is refused by name before anything is created or removed.
#[test]
fn a_project_agent_writes_no_workspace_outside_its_project() {
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
    let (their_owner, their_agent) = project_agent(&mut state, &theirs);
    let their_worker = state
        .agent_action(
            &their_owner,
            &their_agent,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: elsewhere.clone(),
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: None,
                role: None,
                capability: None,
            },
        )
        .expect("the workspace's own project agent puts an agent on it");
    let their_worker = their_worker["agent"]["id"].as_str().unwrap().to_string();

    let (owner, agent_id) = project_agent(&mut state, &mine);
    for action in [
        BridgeAction::AddWorkspaceAgent {
            workspace_id: elsewhere.clone(),
            notify_user: None,
            harness: None,
            model: None,
            effort: None,
            name: None,
            role: None,
            capability: None,
        },
        BridgeAction::RemoveWorkspaceAgent {
            workspace_id: elsewhere.clone(),
            agent_id: their_worker.clone(),
        },
    ] {
        let tool = action.tool_name();
        let refused = state.agent_action(&owner, &agent_id, action).unwrap_err();
        assert!(
            refused.contains(&elsewhere) && refused.contains(&mine),
            "{tool}: {refused}"
        );
    }

    // Nothing was touched over there.
    let still_there = state
        .on_agent_mcp_action(
            &their_owner,
            &their_agent,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: elsewhere,
            },
        )
        .expect("the other project reads its own workspace");
    assert_eq!(still_there["agents"][0]["id"], their_worker);
}

/// A message to a workspace agent goes in as the PROJECT agent's, not the
/// user's: the role is the side it arrives on, and `from_agent` is who wrote
/// it, so the agent reading it knows a machine sent it.
///
/// The operation the post creates remembers the project agent and the
/// conversation it sent from, which is where an answer is owed.
#[test]
fn a_project_agent_messages_a_workspace_agent_as_itself() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "one");
    let (owner, agent_id) = project_agent(&mut state, &project_id);
    let added = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: None,
                role: None,
                capability: None,
            },
        )
        .expect("a project agent staffs its workspace");
    let entity_id = added["entity_id"].as_str().unwrap().to_string();
    let worker = added["agent"]["id"].as_str().unwrap().to_string();

    let delivered = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::MessageWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                agent_id: None,
                body: "start with the router, then the rail".to_string(),
            },
        )
        .expect("a project agent speaks to an agent on its workspace");
    assert_eq!(delivered["workspace_id"], workspace_id);
    assert_eq!(delivered["entity_id"], entity_id);
    assert_eq!(delivered["agent_id"], worker);
    let operation_id = delivered["operation_id"].as_str().unwrap().to_string();

    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": worker }),
    ));
    let sent = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["data"]["body"] == "start with the router, then the rail")
        .unwrap_or_else(|| panic!("the message is on the workspace agent's thread: {page:?}"))
        .clone();
    assert_eq!(sent["data"]["role"], "user", "{sent:?}");
    assert_eq!(sent["data"]["from_agent"]["id"], agent_id, "{sent:?}");

    // The operation remembers who asked for it, and the conversation it asked
    // from — who wanted this, kept as history rather than as an address.
    let receipt = state
        .operation_receipt(&operation_id)
        .expect("the operation is readable")
        .expect("the post created an operation");
    let requested_by = receipt
        .requested_by
        .expect("an agent asked for this operation");
    assert_eq!(requested_by.agent_id, agent_id);
    assert_eq!(requested_by.entity_id, owner);
    assert_eq!(
        requested_by.conversation_id,
        state
            .entity_agents(&owner)
            .unwrap()
            .resolve(Some(&agent_id))
            .unwrap()
            .conversation_id(),
    );

    // A message the human sends down the same path carries no sender at all.
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": entity_id, "agent_id": worker, "body": "and check the tests" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": worker }),
    ));
    let human = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["data"]["body"] == "and check the tests")
        .unwrap_or_else(|| panic!("the human's message is there too: {page:?}"))
        .clone();
    assert!(human["data"]["from_agent"].is_null(), "{human:?}");
}

/// The scope is the binding here too: an agent on another project's workspace
/// is not this project agent's to talk to.
#[test]
fn a_project_agent_messages_no_agent_outside_its_project() {
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
    let (their_owner, their_agent) = project_agent(&mut state, &theirs);
    state
        .agent_action(
            &their_owner,
            &their_agent,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: elsewhere.clone(),
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: None,
                role: None,
                capability: None,
            },
        )
        .expect("the other project staffs its own workspace");

    let (owner, agent_id) = project_agent(&mut state, &mine);
    let refused = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::MessageWorkspaceAgent {
                workspace_id: elsewhere.clone(),
                agent_id: None,
                body: "do my work instead".to_string(),
            },
        )
        .unwrap_err();
    assert!(
        refused.contains(&elsewhere) && refused.contains(&mine),
        "{refused}"
    );
}

// ==== the answer does not come back on its own ============================

/// A project agent, an agent it staffed a workspace with, and the message it
/// handed over — what every hand-off test starts from.
pub(super) struct HandedOver {
    pub(super) owner: String,
    pub(super) agent_id: String,
    pub(super) workspace_id: String,
    pub(super) entity_id: String,
    pub(super) worker: String,
}

pub(super) fn handed_over(state: &mut AppState, project_id: &str, body: &str) -> HandedOver {
    let workspace_id = workspace(state, project_id, "one");
    let (owner, agent_id) = project_agent(state, project_id);
    let added = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: None,
                role: None,
                capability: None,
            },
        )
        .expect("a project agent staffs its workspace");
    let entity_id = added["entity_id"].as_str().unwrap().to_string();
    let worker = added["agent"]["id"].as_str().unwrap().to_string();
    state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::MessageWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                agent_id: None,
                body: body.to_string(),
            },
        )
        .expect("a project agent hands work over");
    HandedOver {
        owner,
        agent_id,
        workspace_id,
        entity_id,
        worker,
    }
}

/// One conversation's items, as the client pages them.
pub(super) fn items(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": agent_id }),
    ));
    page["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a page of items: {page:?}"))
        .clone()
}

/// What one agent said into another agent's conversation.
pub(super) fn sent_by(
    state: &mut AppState,
    entity_id: &str,
    agent_id: &str,
    sender: &str,
) -> Vec<Value> {
    items(state, entity_id, agent_id)
        .into_iter()
        .filter(|item| item["data"]["from_agent"]["id"] == json!(sender))
        .collect()
}

/// When the inbox last recorded the human saying something to an entity.
fn last_user_message(state: &AppState, entity_id: &str) -> Option<String> {
    state
        .board
        .attention()
        .attention(entity_id)
        .and_then(|attention| attention.last_user_message_at.clone())
}

/// A terminal report, the one the MCP server makes from a Complete or Blocked
/// `post_thread_message`.
pub(super) fn terminal(status: DoneStatus, summary: &str) -> DoneReport {
    DoneReport::new(status, summary)
}

/// A terminal message reports to the user, and reaches no agent. The workspace
/// agent ends the turn the project agent started, and the project agent's
/// conversation stays exactly as it was: nothing posted, nothing delivered.
///
/// A reply between agents is an explicit send, so it is the workspace agent's
/// own `message_agent` that carries its answer — and that one does arrive.
#[test]
fn a_workspace_agents_report_reaches_no_agent_on_its_own() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let handed = handed_over(&mut state, &project_id, "read the router");
    let before = items(&mut state, &handed.owner, &handed.agent_id).len();

    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Completed, "the router reads top to bottom"),
    );

    assert!(
        sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker).is_empty(),
        "a report is for the user, not for the agent that asked"
    );
    assert_eq!(
        items(&mut state, &handed.owner, &handed.agent_id).len(),
        before,
        "nothing at all landed in the project agent's conversation"
    );
    assert!(
        !state
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == handed.owner && turn.agent_id == handed.agent_id),
        "and the project agent was not woken for it"
    );

    // Its answer travels the one way an answer travels: the workspace agent
    // says it, to the id the envelope handed it.
    state
        .agent_action(
            &handed.entity_id,
            &handed.worker,
            BridgeAction::MessageAgent {
                agent_id: handed.agent_id.clone(),
                body: "the router reads top to bottom".to_string(),
            },
        )
        .expect("a workspace agent answers the agent that asked");
    let answers = sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(answers[0]["data"]["role"], "user", "{:?}", answers[0]);
    assert_eq!(
        answers[0]["data"]["body"], "the router reads top to bottom",
        "{:?}",
        answers[0]
    );
}

/// The inbox belongs to the human. A project agent handing work over is the
/// work happening, so it does not cross the line the human drew on a row, and
/// a report the workspace agent makes reaches no row but its own.
#[test]
fn a_hand_off_brings_back_no_cleared_row() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let handed = handed_over(&mut state, &project_id, "read the router");
    let quiet = last_user_message(&state, &handed.owner);
    for entity_id in [&handed.entity_id, &handed.owner] {
        let cleared = state.handle(req("entity.dismiss", json!({ "entity_id": entity_id })));
        assert_eq!(cleared["ok"], true, "{cleared:?}");
    }

    // Out: the project agent hands over more work. The board is what discards
    // a dismissal whose line was crossed, so it is read before the row is
    // judged.
    state
        .agent_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::MessageWorkspaceAgent {
                workspace_id: handed.workspace_id.clone(),
                agent_id: None,
                body: "and the rail after it".to_string(),
            },
        )
        .expect("a project agent speaks to an agent on its workspace");
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "{board:?}");
    assert!(
        state.is_dismissed(&handed.entity_id),
        "a message a machine sent is not the row speaking"
    );
    assert_eq!(
        last_user_message(&state, &handed.owner),
        quiet,
        "and it is nobody's unread"
    );

    // The workspace agent ends its turn. Its own conversation calls the human —
    // it stopped and said so — and the project agent's row hears nothing.
    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Completed, "the router reads top to bottom"),
    );
    state.handle(req("board.list", json!({})));
    assert!(
        !state.is_dismissed(&handed.entity_id),
        "the agent handing its turn back is the row speaking"
    );
    assert!(
        state.is_dismissed(&handed.owner),
        "but it said nothing to the agent that asked"
    );

    // And the human's own words still cross the line that was kept.
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": handed.owner, "agent_id": handed.agent_id, "body": "thanks" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    state.handle(req("board.list", json!({})));
    assert!(
        !state.is_dismissed(&handed.owner),
        "the dismissal was kept, not discarded, so the human can cross it"
    );
    assert_ne!(
        last_user_message(&state, &handed.owner),
        quiet,
        "the human's own message does move it"
    );
}

/// A mid-turn message reaches no agent either, for the same reason a terminal
/// one does not: what an agent says with `post_thread_message` is the user's
/// to read, whatever its status.
#[test]
fn a_mid_turn_message_reaches_no_agent_either() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let handed = handed_over(&mut state, &project_id, "read the router");

    for body in ["still reading", "which router did you mean?"] {
        state
            .agent_action(
                &handed.entity_id,
                &handed.worker,
                BridgeAction::PostThreadMessage {
                    still_working: body == "still reading",
                    body: body.to_string(),
                    options: Vec::new(),
                },
            )
            .expect("a workspace agent speaks mid-turn");
    }
    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Blocked, "the router is three files"),
    );
    assert!(
        sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker).is_empty(),
        "Working, Waiting and Blocked are all the user's to read"
    );
}

/// The project agent manages the project's folders and its workspaces'
/// directories, through the same verbs the client calls. Which project is
/// written comes from the owner binding, so `add_project_source` carries no
/// project id at all.
#[test]
fn a_project_agent_adds_and_removes_folders_on_its_own_project() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let assets = state_root.join("assets");
    std::fs::create_dir(&assets).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let added = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddProjectSource {
                path: Some(assets.display().to_string()),
                remote: None,
                name: Some("assets".to_string()),
                base_branch: None,
            },
        )
        .expect("a project agent adds a folder to its own project");
    assert_eq!(added["project_id"], project_id, "{added:?}");
    let source_id = added["sources"][1]["id"].as_str().unwrap().to_string();
    assert_eq!(added["sources"][1]["name"], "assets");

    let removed = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::RemoveProjectSource {
                source_id: source_id.clone(),
            },
        )
        .expect("and takes it off again");
    assert_eq!(
        removed["sources"].as_array().unwrap().len(),
        1,
        "{removed:?}"
    );

    let refused = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::RemoveProjectSource {
                source_id: "source-9".to_string(),
            },
        )
        .expect_err("a source the project does not have is refused");
    assert!(refused.contains("unknown source_id"), "{refused}");
}

/// Every workspace tool passes the same gate: the id is checked against the
/// owner's binding before anything runs.
#[test]
fn a_project_agent_changes_only_its_own_projects_workspaces() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let assets = state_root.join("assets");
    std::fs::create_dir(&assets).unwrap();
    let mut state = rooted(&state_root);
    let mine = added_project(&mut state, &repo);
    let theirs = added_project(&mut state, &other_repo);
    let elsewhere = workspace(&mut state, &theirs, "theirs");
    let ours = workspace(&mut state, &mine, "ours");
    let (owner, agent_id) = project_agent(&mut state, &mine);

    let grown = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddWorkspaceDirectory {
                workspace_id: ours.clone(),
                source_id: None,
                path: Some(assets.display().to_string()),
                remote: None,
                name: Some("assets".to_string()),
            },
        )
        .expect("a project agent adds a directory to its own workspace");
    let directories = grown["directories"].as_array().unwrap();
    assert_eq!(directories.len(), 2, "{grown:?}");
    let directory_id = directories[1]["id"].as_str().unwrap().to_string();

    let shrunk = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::RemoveWorkspaceDirectory {
                workspace_id: ours.clone(),
                directory_id,
            },
        )
        .expect("and takes it off again");
    assert_eq!(
        shrunk["directories"].as_array().unwrap().len(),
        1,
        "{shrunk:?}"
    );

    let deleted = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::DeleteWorkspace {
                workspace_id: ours.clone(),
            },
        )
        .expect("a project agent deletes its own workspace");
    assert_eq!(deleted["deleted"], true, "{deleted:?}");
    let listed = state.handle(req("workspace.list", json!({"project_id": mine})));
    assert!(
        !listed["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["workspace_id"] == json!(ours)),
        "{listed:?}"
    );

    for action in [
        BridgeAction::DeleteWorkspace {
            workspace_id: elsewhere.clone(),
        },
        BridgeAction::AddWorkspaceDirectory {
            workspace_id: elsewhere.clone(),
            source_id: None,
            path: Some(assets.display().to_string()),
            remote: None,
            name: None,
        },
        BridgeAction::RemoveWorkspaceDirectory {
            workspace_id: elsewhere.clone(),
            directory_id: "whatever".to_string(),
        },
    ] {
        let name = action.tool_name();
        let refused = state
            .agent_action(&owner, &agent_id, action)
            .expect_err("a workspace in another project is refused");
        assert!(
            refused.contains(&format!("workspace {elsewhere} is not in project {mine}")),
            "{name}: {refused}"
        );
    }
}

// ==== the project's own conversation is not a workspace ====================

/// A project's conversation owner stands in a scratch directory, not in a
/// checkout — so it is not a workspace, and nothing that lists workspaces may
/// hand it back.
///
/// It used to: legacy adoption imports every run with no base branch whose root
/// is not already a workspace root, and the project's owner is exactly that.
/// The project agent then read itself out of `list_workspaces`, listed its own
/// agents, and sent itself a message.
#[test]
fn a_projects_conversation_owner_is_never_one_of_its_own_workspaces() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let real = workspace(&mut state, &project_id, "one");
    let (owner, agent_id) = project_agent(&mut state, &project_id);
    let scratch = state_root.join(crate::app::projects::PROJECT_SCRATCH_DIR_NAME);

    let listed = state.handle(req("workspace.list", json!({ "project_id": project_id })));
    let workspaces = listed["result"]["workspaces"].as_array().unwrap().clone();
    for workspace in &workspaces {
        let root = workspace["root"].as_str().unwrap_or_default();
        assert!(
            !std::path::Path::new(root).starts_with(&scratch),
            "the project's scratch root is not a workspace: {workspace:?}"
        );
        assert_ne!(
            workspace["workspace_id"],
            json!(owner),
            "the project's conversation owner is not a workspace: {workspace:?}"
        );
    }
    assert!(
        workspaces
            .iter()
            .any(|workspace| workspace["workspace_id"] == json!(real)),
        "the real workspace is still there: {listed:?}"
    );

    // The agent's own read answers the same list, so it never finds itself.
    let mine = state
        .on_agent_mcp_action(&owner, &agent_id, BridgeAction::ListWorkspaces)
        .expect("a project agent reads its own workspaces");
    assert_eq!(mine["workspaces"], json!(workspaces), "{mine:?}");

    // And no workspace, real or invented, answers the project owner's roster.
    for workspace_id in [real.as_str(), owner.as_str()] {
        let agents = state.on_agent_mcp_action(
            &owner,
            &agent_id,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: workspace_id.to_string(),
            },
        );
        let listed = agents.unwrap_or_else(|_| json!({ "agents": [] }));
        assert!(
            !listed["agents"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|agent| agent["id"] == json!(agent_id)),
            "{workspace_id}: a project agent is never a workspace agent: {listed:?}"
        );
    }
}

// ==== where a message came from ===========================================

/// The name a project answers to, as its own list gives it.
fn project_name(state: &mut AppState, project_id: &str) -> String {
    let listed = state.handle(req("project.list", json!({})));
    listed["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["project_id"] == json!(project_id))
        .unwrap_or_else(|| panic!("the project is listed: {listed:?}"))["name"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A message from an agent says where it was sent from: the workspace or the
/// project its conversation belongs to, and what that conversation is about.
/// Both ends of a hand-off are stamped — the instruction going out and the
/// answer sent back — so a client can draw and link either one without a
/// second read.
#[test]
fn a_message_from_an_agent_names_the_conversation_it_came_from() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let name = project_name(&mut state, &project_id);
    let handed = handed_over(&mut state, &project_id, "read the router");
    state
        .on_agent_mcp_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::SetTopic {
                topic: "Staffing the rail".to_string(),
            },
        )
        .expect("a project agent names its conversation");
    state
        .on_agent_mcp_action(
            &handed.entity_id,
            &handed.worker,
            BridgeAction::SetTopic {
                topic: "Reading the router".to_string(),
            },
        )
        .expect("a workspace agent names its conversation");

    // Sent after both topics are set, so the stamp is what was true at send.
    state
        .agent_action(
            &handed.owner,
            &handed.agent_id,
            BridgeAction::MessageWorkspaceAgent {
                workspace_id: handed.workspace_id.clone(),
                agent_id: None,
                body: "and then the rail".to_string(),
            },
        )
        .expect("a project agent hands more work over");

    let inbound = items(&mut state, &handed.entity_id, &handed.worker)
        .into_iter()
        .find(|item| item["data"]["body"] == json!("and then the rail"))
        .expect("the instruction is on the workspace agent's thread");
    assert_eq!(
        inbound["data"]["from_agent"],
        json!({
            "id": handed.agent_id,
            "owner": { "kind": "project", "id": project_id, "name": name },
            "topic": "Staffing the rail",
        }),
        "{inbound:?}"
    );

    state
        .agent_action(
            &handed.entity_id,
            &handed.worker,
            BridgeAction::MessageAgent {
                agent_id: handed.agent_id.clone(),
                body: "the router reads top to bottom".to_string(),
            },
        )
        .expect("a workspace agent answers the agent that asked");

    let answers = sent_by(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(
        answers[0]["data"]["from_agent"],
        json!({
            "id": handed.worker,
            "owner": { "kind": "workspace", "id": handed.workspace_id, "name": "one" },
            "topic": "Reading the router",
        }),
        "{:?}",
        answers[0]
    );
}
