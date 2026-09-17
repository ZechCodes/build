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
    assert!(!coding.contains("list_workspaces"), "{coding}");
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
                anchor: None,
                links: Vec::new(),
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

    let ours = state.handle(req("workspace.list", json!({ "project_id": mine })));
    let names: Vec<&str> = ours["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"read the router"), "{ours:?}");

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
                harness: None,
                model: None,
                effort: None,
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
                harness: None,
                model: None,
                effort: None,
            },
        )
        .expect("the workspace's own project agent puts an agent on it");
    let their_worker = their_worker["agent"]["id"].as_str().unwrap().to_string();

    let (owner, agent_id) = project_agent(&mut state, &mine);
    for action in [
        BridgeAction::AddWorkspaceAgent {
            workspace_id: elsewhere.clone(),
            harness: None,
            model: None,
            effort: None,
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
                harness: None,
                model: None,
                effort: None,
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

    // The operation remembers who asked for it, and the conversation the answer
    // is owed to — which is what a reply is forwarded along.
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
                harness: None,
                model: None,
                effort: None,
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

// ==== the answer coming back ==============================================

/// A project agent, an agent it staffed a workspace with, and the message it
/// handed over — what every forwarding test starts from.
struct HandedOver {
    owner: String,
    agent_id: String,
    entity_id: String,
    worker: String,
}

fn handed_over(state: &mut AppState, project_id: &str, body: &str) -> HandedOver {
    let workspace_id = workspace(state, project_id, "one");
    let (owner, agent_id) = project_agent(state, project_id);
    let added = state
        .agent_action(
            &owner,
            &agent_id,
            BridgeAction::AddWorkspaceAgent {
                workspace_id: workspace_id.clone(),
                harness: None,
                model: None,
                effort: None,
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
                workspace_id,
                agent_id: None,
                body: body.to_string(),
            },
        )
        .expect("a project agent hands work over");
    HandedOver {
        owner,
        agent_id,
        entity_id,
        worker,
    }
}

/// One conversation's items, as the client pages them.
fn items(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
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
fn forwarded(state: &mut AppState, entity_id: &str, agent_id: &str, sender: &str) -> Vec<Value> {
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
fn terminal(status: DoneStatus, summary: &str) -> DoneReport {
    DoneReport {
        phase: DonePhase::Build,
        status,
        summary: summary.to_string(),
        outputs: DoneOutputs::default(),
    }
}

/// The workspace agent finishes the turn the project agent started, and its
/// terminal message is handed back: the project agent's own conversation, on
/// the user's side of it, wearing the agent that wrote it and saying how the
/// turn ended. The project agent needs no tool for this and never learns it
/// was summoned by a machine.
#[test]
fn a_workspace_agents_answer_reaches_the_project_agent_that_asked() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let handed = handed_over(&mut state, &project_id, "read the router");
    let quiet = last_user_message(&state, &handed.owner);

    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Completed, "the router reads top to bottom"),
    );

    let answers = forwarded(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    assert_eq!(answers.len(), 1, "one answer, forwarded once: {answers:?}");
    let answer = &answers[0];
    assert_eq!(answer["data"]["role"], "user", "{answer:?}");
    let body = answer["data"]["body"].as_str().unwrap();
    assert!(body.starts_with("Complete."), "{body}");
    assert!(body.contains("the router reads top to bottom"), "{body}");

    // Delivered the way `agent.deliver` delivers one: a turn for the project
    // agent, carrying the answer itself rather than a fetch instruction.
    let turn = state
        .delivery_queue
        .queued()
        .find(|turn| turn.owner == handed.owner && turn.agent_id == handed.agent_id)
        .unwrap_or_else(|| panic!("the project agent has a turn waiting"));
    let say = turn.say.as_ref().expect("the turn says something");
    assert!(
        say.warm.contains("the router reads top to bottom"),
        "{say:?}"
    );
    assert!(
        say.warm
            .contains(&format!("came from agent `{}`", handed.worker)),
        "{say:?}"
    );

    // The human was not here. One agent answering another is the work
    // happening, and it must not move the inbox anchor under the reader.
    assert_eq!(
        last_user_message(&state, &handed.owner),
        quiet,
        "a forwarded answer is nobody's unread"
    );
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": handed.owner, "agent_id": handed.agent_id, "body": "thanks" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_ne!(
        last_user_message(&state, &handed.owner),
        quiet,
        "the human's own message does move it"
    );
}

/// The loop guard. A forwarded answer is never itself forwarded, and the debt
/// it settled is settled once: the agent that received it owes nobody, so its
/// own terminal message goes nowhere, and a second report from the workspace
/// agent answers a question nobody asked.
#[test]
fn an_answer_handed_back_is_not_handed_on() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let handed = handed_over(&mut state, &project_id, "read the router");

    state.on_agent_done(&handed.entity_id, terminal(DoneStatus::Completed, "done"));
    let before = items(&mut state, &handed.entity_id, &handed.worker).len();

    // The project agent's turn ends the same way, and answers nobody: the
    // message that started it arrived by forwarding and owes no reply.
    state.forward_terminal_reply(
        &handed.owner,
        &handed.agent_id,
        &terminal(DoneStatus::Completed, "I will tell the user"),
    );
    assert_eq!(
        items(&mut state, &handed.entity_id, &handed.worker).len(),
        before,
        "nothing goes back the way it came"
    );

    // And the workspace agent's next turn is its own: one message, one answer.
    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Completed, "and the rail too"),
    );
    assert_eq!(
        forwarded(&mut state, &handed.owner, &handed.agent_id, &handed.worker).len(),
        1,
        "the answer was owed once"
    );
}

/// Only a terminal message is an answer. A progress note or a question keeps
/// the turn open, so nothing is handed back and the project agent is not
/// spammed mid-turn — the debt is still standing when the turn really ends.
#[test]
fn a_mid_turn_message_is_not_an_answer() {
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
                    anchor: None,
                    links: Vec::new(),
                    options: Vec::new(),
                },
            )
            .expect("a workspace agent speaks mid-turn");
    }
    assert!(
        forwarded(&mut state, &handed.owner, &handed.agent_id, &handed.worker).is_empty(),
        "Working and Waiting are not answers"
    );

    state.on_agent_done(
        &handed.entity_id,
        terminal(DoneStatus::Blocked, "the router is three files"),
    );
    let answers = forwarded(&mut state, &handed.owner, &handed.agent_id, &handed.worker);
    assert_eq!(answers.len(), 1, "{answers:?}");
    let body = answers[0]["data"]["body"].as_str().unwrap();
    assert!(body.starts_with("Blocked."), "{body}");
    assert!(body.contains("the router is three files"), "{body}");
}
