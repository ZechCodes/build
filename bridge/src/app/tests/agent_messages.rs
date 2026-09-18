//! One agent speaking to another, on every surface that has a conversation.
//!
//! The scope is the project: an agent reaches the agents of conversation
//! owners bound to the same project as its own owner, and nothing else — not
//! another project's agents, not an id that names nobody, and not itself.

use super::project_agent::{added_project, forwarded, items, project_agent, rooted, workspace};
use super::*;
use crate::mcp::{BridgeAction, McpSurface};

/// A coding agent on one workspace of a project, and the conversation owner it
/// belongs to.
fn workspace_agent(state: &mut AppState, workspace_id: &str) -> (String, String) {
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": owner })));
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    (owner, agent_id)
}

fn message_agent(agent_id: &str, body: &str) -> BridgeAction {
    BridgeAction::MessageAgent {
        agent_id: agent_id.to_string(),
        body: body.to_string(),
    }
}

/// Every agent can reach every other agent in its project, whichever surface it
/// is on. The message arrives as the sender's — the user's role, because that
/// is the side an instruction arrives on whoever wrote it, wearing the sender —
/// and the operation remembers the sender, so the reply comes back the way a
/// project agent's does.
#[test]
fn one_coding_agent_messages_another_in_the_same_project() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let one = workspace(&mut state, &project_id, "one");
    let two = workspace(&mut state, &project_id, "two");
    let (sender_owner, sender) = workspace_agent(&mut state, &one);
    let (target_owner, target) = workspace_agent(&mut state, &two);

    let sent = state
        .agent_action(
            &sender_owner,
            &sender,
            message_agent(&target, "the rail moved; rebase on main"),
        )
        .expect("an agent reaches another agent in its project");
    assert_eq!(sent["agent_id"], json!(target), "{sent:?}");
    assert_eq!(sent["entity_id"], json!(target_owner), "{sent:?}");
    let operation_id = sent["operation_id"].as_str().unwrap().to_string();

    let inbound = items(&mut state, &target_owner, &target)
        .into_iter()
        .find(|item| item["data"]["body"] == json!("the rail moved; rebase on main"))
        .expect("the message is on the target's thread");
    assert_eq!(inbound["data"]["role"], "user", "{inbound:?}");
    assert_eq!(
        inbound["data"]["from_agent"],
        json!({
            "id": sender,
            "owner": { "kind": "workspace", "id": one, "name": "one" },
            "topic": "",
        }),
        "{inbound:?}"
    );

    // Recorded as a requester, so the target's terminal report comes back.
    let requested_by = state
        .operation_receipt(&operation_id)
        .expect("the operation is readable")
        .expect("the post created an operation")
        .requested_by
        .expect("an agent asked for this operation");
    assert_eq!(requested_by.agent_id, sender);
    assert_eq!(requested_by.entity_id, sender_owner);

    state.on_agent_done(
        &target_owner,
        super::project_agent::terminal(crate::mcp::DoneStatus::Completed, "rebased"),
    );
    let answers = forwarded(&mut state, &sender_owner, &sender, &target);
    assert_eq!(answers.len(), 1, "the report comes back: {answers:?}");
    assert_eq!(
        answers[0]["data"]["from_agent"]["owner"],
        json!({ "kind": "workspace", "id": two, "name": "two" }),
        "{:?}",
        answers[0]
    );
}

/// An agent cannot message itself. The refusal names the id it was given and
/// says why, on every surface that has the tool.
#[test]
fn no_agent_messages_itself() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let one = workspace(&mut state, &project_id, "one");
    let (coder_owner, coder) = workspace_agent(&mut state, &one);
    let (project_owner, project) = project_agent(&mut state, &project_id);

    for (owner, agent) in [(&coder_owner, &coder), (&project_owner, &project)] {
        let refused = state
            .agent_action(owner, agent, message_agent(agent, "do it yourself"))
            .expect_err("an agent cannot message itself");
        assert!(
            refused.contains(agent) && refused.contains("cannot message itself"),
            "{refused}"
        );
    }

    // And nothing landed.
    assert!(
        items(&mut state, &coder_owner, &coder)
            .iter()
            .all(|item| item["data"]["body"] != json!("do it yourself")),
        "nothing was posted"
    );
}

/// The scope is the project on both ends: an agent of another project's
/// conversation owner is refused by name, and so is an id that names nobody.
#[test]
fn an_agent_reaches_no_agent_outside_its_own_project() {
    let (_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let mine = added_project(&mut state, &repo);
    let theirs = added_project(&mut state, &other_repo);
    let here = workspace(&mut state, &mine, "here");
    let elsewhere = workspace(&mut state, &theirs, "elsewhere");
    let (owner, sender) = workspace_agent(&mut state, &here);
    let (_their_owner, theirs_agent) = workspace_agent(&mut state, &elsewhere);

    let refused = state
        .agent_action(&owner, &sender, message_agent(&theirs_agent, "do my work"))
        .expect_err("another project's agent is not reachable");
    assert!(
        refused.contains(&theirs_agent) && refused.contains(&mine),
        "{refused}"
    );

    let unknown = state
        .agent_action(&owner, &sender, message_agent("agent-nobody", "hello"))
        .expect_err("an id that names nobody is refused");
    assert!(unknown.contains("agent-nobody"), "{unknown}");
}

/// The tool is on both surfaces that have a conversation, and on neither of the
/// others — and the inventory each surface advertises is what the socket lets
/// through, which is what Codex's allow-list is built from.
#[test]
fn message_agent_is_on_the_coding_and_project_surfaces() {
    let action = BridgeAction::MessageAgent {
        agent_id: "agent-2".to_string(),
        body: "hello".to_string(),
    };
    assert_eq!(action.tool_name(), "message_agent");
    assert!(action.allowed_on(McpSurface::Coding));
    assert!(action.allowed_on(McpSurface::Project));
    assert!(!action.allowed_on(McpSurface::Router));

    for surface in [McpSurface::Coding, McpSurface::Project] {
        assert!(
            crate::mcp::DoneServer::tool_names_of(surface).contains(&"message_agent".to_string()),
            "{surface:?}"
        );
    }
    assert!(!crate::mcp::DoneServer::tool_names_of(McpSurface::Router)
        .contains(&"message_agent".to_string()));
}
