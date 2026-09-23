//! The tracker's eight tools, on both working surfaces (spec: Issues → The
//! MCP tools).
//!
//! What the tests here are about is the scope: the project comes from the
//! calling agent and never from an argument, the author is the calling agent
//! and never a claim, and neither can be widened by how a call is spelled.

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::{BridgeAction, DoneServer, McpSurface};

/// A coding agent on a workspace of this project, as `(entity_id, agent_id)`.
fn coding_agent(state: &mut AppState, project_id: &str, name: &str) -> (String, String) {
    let workspace_id = workspace(state, project_id, name);
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    assert_eq!(conversation["ok"], true, "{conversation:?}");
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    (entity_id, agent_id)
}

fn call(
    state: &mut AppState,
    who: &(String, String),
    action: BridgeAction,
) -> Result<Value, String> {
    state.on_agent_mcp_action(&who.0, &who.1, action)
}

/// The eleven the tracker adds, by name.
const ISSUE_TOOLS: [&str; 11] = [
    "list_issues",
    "get_issue",
    "read_comment",
    "create_issue",
    "comment_issue",
    "assign_issue",
    "move_issue",
    "close_issue",
    "link_issue",
    "track_issue",
    "untrack_issue",
];

/// Both working surfaces are shown the same eleven tools. The router is shown
/// none of them: it has no project to be scoped to.
#[test]
fn both_working_surfaces_carry_the_issue_tools_and_the_router_carries_none() {
    for surface in [McpSurface::Coding, McpSurface::Project] {
        let listed = DoneServer::tool_names_of(surface);
        for tool in ISSUE_TOOLS {
            assert!(
                listed.contains(&tool.to_string()),
                "{surface:?} is not shown {tool}: {listed:?}"
            );
        }
    }
    let router = DoneServer::tool_names_of(McpSurface::Router);
    for tool in ISSUE_TOOLS {
        // `create_issue` is the ROUTER's own, and the plan flow's — the same
        // name on a disjoint surface, the way `post_thread_message` already is.
        if tool == "create_issue" {
            continue;
        }
        assert!(
            !router.contains(&tool.to_string()),
            "the router is shown {tool}: {router:?}"
        );
    }
}

/// A tool a session's surface does not carry is refused on the socket too, so
/// a harness writing its own frames reaches no further than one reading the
/// list.
#[test]
fn the_socket_refuses_an_issue_tool_from_the_router() {
    let action = BridgeAction::TrackerListIssues {
        state: None,
        status: None,
        label: None,
    };
    assert!(action.allowed_on(McpSurface::Coding));
    assert!(action.allowed_on(McpSurface::Project));
    assert!(
        !action.allowed_on(McpSurface::Router),
        "the router has no project to be scoped to"
    );
}

/// A coding agent reads, files and moves the issues of its own project, and
/// what it writes is signed by it.
#[test]
fn a_coding_agent_files_an_issue_signed_by_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let filed_by_agent = call(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
            title: "The drop handler races the column read".into(),
            body: Some("Found while fixing something else.".into()),
            status: None,
            labels: vec!["bug".into()],
            priority: Some("high".into()),
            track: None,
            attachments: Vec::new(),
            notify_user: None,
        },
    )
    .expect("an agent files an issue in its own project");
    let issue = &filed_by_agent["issue"];
    assert_eq!(issue["project_id"], project_id.as_str());
    assert_eq!(
        issue["created_by"],
        json!({ "kind": "agent", "agent_id": who.1 }),
        "the author is who called, not who claimed"
    );
    assert_eq!(issue["labels"], json!(["bug"]));
    assert_eq!(issue["priority"], "high");
    assert_eq!(issue["status"], "backlog");

    // And the client sees the same issue, because it is the same record.
    let listed = state.handle(req("issues.list", json!({ "project_id": project_id })));
    assert_eq!(
        listed["result"]["issues"][0]["id"], issue["id"],
        "{listed:?}"
    );
}

/// A comment and an event are signed by the calling agent.
#[test]
fn a_comment_and_a_move_are_signed_by_the_agent_that_made_them() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = filed(&mut state, &project_id, "one")["id"]
        .as_str()
        .unwrap()
        .to_string();

    call(
        &mut state,
        &who,
        BridgeAction::TrackerCommentIssue {
            issue_id: id.clone(),
            body: "Reproduced it.".into(),
            refs: Vec::new(),
            track: None,
            attachments: Vec::new(),
            notify_user: None,
            mention_user: None,
        },
    )
    .expect("an agent comments");
    call(
        &mut state,
        &who,
        BridgeAction::TrackerMoveIssue {
            issue_id: id.clone(),
            status: "in_review".into(),
            track: None,
        },
    )
    .expect("an agent moves its issue");

    let timeline = state.handle(req("issues.get", json!({ "issue_id": id })));
    let entries = timeline["result"]["timeline"].as_array().unwrap();
    let comment = entries
        .iter()
        .find(|entry| entry["type"] == "comment")
        .expect("the comment is on the timeline");
    assert_eq!(
        comment["author"],
        json!({ "kind": "agent", "agent_id": who.1 })
    );
    let moved = entries
        .iter()
        .find(|entry| entry["kind"] == "moved")
        .expect("the move is on the timeline");
    assert_eq!(
        moved["actor"],
        json!({ "kind": "agent", "agent_id": who.1 })
    );
    assert_eq!(
        timeline["result"]["issue"]["status"], "in_review",
        "Complete means ready to be looked at"
    );
}

/// A real MCP frame carries the mention through the action, durable record,
/// issue timeline, and read_comment answer. Mentioning also watches the issue.
#[test]
fn mcp_comment_mention_round_trips_through_the_bridge() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    state.handle(req(
        "settings.set",
        json!({ "watch_agent_filed_issues": false }),
    ));
    let created = call(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
            title: "Needs a choice".into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            attachments: Vec::new(),
            track: None,
            notify_user: None,
        },
    )
    .unwrap();
    let issue_id = created["issue"]["id"].as_str().unwrap();
    assert_ne!(created["issue"]["watched"], true);

    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "comment_issue", "arguments": {
            "issue_id": issue_id, "body": "Which option should I use?",
            "mention_user": true
        }}
    });
    let parsed = DoneServer::new(&who.1).handle_message(&frame.to_string());
    let action = parsed.action.expect("MCP frame emits a comment action");
    assert!(matches!(
        &action,
        BridgeAction::TrackerCommentIssue {
            mention_user: Some(true),
            ..
        }
    ));
    let commented = call(&mut state, &who, action).unwrap();
    assert_eq!(commented["comment"]["mentions_user"], true);
    assert_eq!(commented["issue"]["watched"], true);
    let comment_id = commented["comment"]["id"].as_str().unwrap();

    let timeline = call(
        &mut state,
        &who,
        BridgeAction::TrackerGetIssue {
            issue_id: issue_id.into(),
        },
    )
    .unwrap();
    assert_eq!(
        timeline["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == comment_id)
            .unwrap()["mentions_user"],
        true
    );
    let read = call(
        &mut state,
        &who,
        BridgeAction::TrackerReadComment {
            comment_id: comment_id.into(),
        },
    )
    .unwrap();
    assert_eq!(read["mentions_user"], true);

    let quiet_frame = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "comment_issue", "arguments": {
            "issue_id": issue_id, "body": "Routine update."
        }}
    });
    let quiet_action = DoneServer::new(&who.1)
        .handle_message(&quiet_frame.to_string())
        .action
        .expect("MCP frame emits a quiet comment action");
    let quiet = call(&mut state, &who, quiet_action).unwrap();
    assert!(quiet["comment"].get("mentions_user").is_none());
    let timeline = state.handle(req("issues.get", json!({ "issue_id": issue_id })));
    assert!(
        timeline["result"]["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == quiet["comment"]["id"]
                && entry.get("mentions_user").is_none())
    );
}

/// An issue of another project is unknown to this agent — not forbidden.
/// It cannot list it and cannot have been handed it, and saying an id it
/// guessed exists somewhere else tells it more than it asked.
#[test]
fn an_issue_of_another_project_is_unknown_to_this_agents_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    let theirs = filed(&mut state, &other_project, "not yours")["id"]
        .as_str()
        .unwrap()
        .to_string();

    for action in [
        BridgeAction::TrackerGetIssue {
            issue_id: theirs.clone(),
        },
        BridgeAction::TrackerMoveIssue {
            issue_id: theirs.clone(),
            status: "done".into(),
            track: None,
        },
        BridgeAction::TrackerCloseIssue {
            issue_id: theirs.clone(),
            reason: None,
            track: None,
        },
        BridgeAction::TrackerCommentIssue {
            issue_id: theirs.clone(),
            body: "mine now".into(),
            refs: Vec::new(),
            track: None,
            attachments: Vec::new(),
            notify_user: None,
            mention_user: None,
        },
    ] {
        let name = action.tool_name();
        let refused = call(&mut state, &who, action).expect_err("another project's issue");
        assert!(refused.contains("unknown issue_id"), "{name}: {refused}");
    }

    // And the other project's issue is untouched.
    let still = state.handle(req("issues.get", json!({ "issue_id": theirs })));
    assert_eq!(still["result"]["issue"]["state"], "open");
    assert_eq!(still["result"]["issue"]["status"], "backlog");
}

/// `list_issues` answers this agent's project and no other, whatever the call
/// carries — there is no project argument for one to come in on.
#[test]
fn list_issues_answers_this_agents_own_project_only() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let mine = filed(&mut state, &project_id, "mine")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    filed(&mut state, &other_project, "theirs");

    let listed = call(
        &mut state,
        &who,
        BridgeAction::TrackerListIssues {
            state: None,
            status: None,
            label: None,
        },
    )
    .expect("an agent lists its own project's issues");
    let ids: Vec<&str> = listed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![mine.as_str()], "only this project's");
    assert_eq!(listed["project_id"], project_id.as_str());
}

/// A project agent gets the same eight tools over the same code path, so the
/// board an agent runs and the board the client reads are one board.
#[test]
fn a_project_agent_runs_the_same_board_the_client_reads() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (owner, agent_id) = project_agent(&mut state, &project_id);
    let who = (owner, agent_id.clone());

    let made = call(
        &mut state,
        &who,
        BridgeAction::TrackerCreateIssue {
            title: "Something the project agent noticed".into(),
            body: None,
            status: Some("ready".into()),
            labels: Vec::new(),
            priority: None,
            track: None,
            attachments: Vec::new(),
            notify_user: None,
        },
    )
    .expect("the project agent files an issue");
    assert_eq!(
        made["issue"]["created_by"],
        json!({ "kind": "agent", "agent_id": agent_id })
    );
    assert_eq!(made["issue"]["status"], "ready");

    let client = state.handle(req("issues.list", json!({ "project_id": project_id })));
    assert_eq!(
        client["result"]["issues"][0]["id"], made["issue"]["id"],
        "one board: {client:?}"
    );
}

/// An agent hands work off by assigning, and the issue records where it went.
#[test]
fn an_agent_hands_an_issue_to_another_agent_of_its_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let them = coding_agent(&mut state, &project_id, "there");
    let id = filed(&mut state, &project_id, "hand this over")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let handed = call(
        &mut state,
        &who,
        BridgeAction::TrackerAssignIssue {
            issue_id: id.clone(),
            assignee: json!({ "kind": "agent", "agent_id": them.1 }),
            note: Some("the parser is the part that matters".into()),
            track: None,
            notify_user: None,
        },
    )
    .expect("an agent assigns to another agent of its project");
    assert_eq!(
        handed["issue"]["assignee"],
        json!({ "kind": "agent", "agent_id": them.1 })
    );
    assert_eq!(handed["dispatch"]["kind"], "agent");
    assert_eq!(handed["issue"]["status"], "in_progress");

    // The delivered message wears the issue AND the agent that sent it, so the
    // reader knows both what the work is and who to answer.
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": them.0, "agent_id": them.1, "limit": 20 }),
    ));
    let handed_over = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .find(|message| message["from_issue"].is_object())
        .unwrap_or_else(|| panic!("no message wearing the issue: {page:?}"));
    assert_eq!(handed_over["from_issue"]["issue_id"], id.as_str());
    assert_eq!(
        handed_over["from_agent"]["id"], who.1,
        "an agent assigned it, so the reader knows who to answer"
    );
    assert!(handed_over["body"]
        .as_str()
        .unwrap()
        .contains("the parser is the part that matters"));
}

#[test]
fn an_agent_assigning_an_issue_can_choose_whether_the_new_agent_is_watched() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let workspace_id = workspace(&mut state, &project_id, "workers");

    for (notify_user, watched) in [(None, false), (Some(true), true)] {
        let id = filed(&mut state, &project_id, "new worker")["id"]
            .as_str()
            .unwrap()
            .to_string();
        let mut assignee = json!({ "kind": "new_agent", "workspace_id": workspace_id });
        if let Some(notify_user) = notify_user {
            assignee["notify_user"] = json!(notify_user);
        }
        let assigned = call(
            &mut state,
            &caller,
            BridgeAction::TrackerAssignIssue {
                issue_id: id,
                assignee,
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("the MCP call assigns to a new agent");
        let entity_id = assigned["dispatch"]["entity_id"].as_str().unwrap();
        let agent_id = assigned["dispatch"]["agent_id"].as_str().unwrap();
        let agent = state.runs[entity_id].agents.by_id(agent_id).unwrap();
        assert_eq!(agent.watched, watched, "{assigned:?}");
    }
}

#[test]
fn an_agent_assigning_an_issue_can_watch_a_new_workspace_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = filed(&mut state, &project_id, "new workspace worker")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assigned = state
        .agent_action(
            &caller.0,
            &caller.1,
            BridgeAction::TrackerAssignIssue {
                issue_id: id,
                assignee: json!({
                    "kind": "new_workspace",
                    "isolation": "worktree",
                    "notify_user": true,
                }),
                note: None,
                track: None,
                notify_user: None,
            },
        )
        .expect("the MCP call cuts a workspace and assigns its agent");
    let entity_id = assigned["dispatch"]["entity_id"].as_str().unwrap();
    let agent_id = assigned["dispatch"]["agent_id"].as_str().unwrap();
    assert!(
        state.runs[entity_id]
            .agents
            .by_id(agent_id)
            .unwrap()
            .watched
    );
}

/// An agent of another project cannot be assigned to, and the issue is left
/// exactly as it was.
#[test]
fn an_agent_cannot_hand_an_issue_outside_its_own_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = filed(&mut state, &project_id, "one")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    let (_owner, foreign) = project_agent(&mut state, &other_project);

    let refused = call(
        &mut state,
        &who,
        BridgeAction::TrackerAssignIssue {
            issue_id: id.clone(),
            assignee: json!({ "kind": "agent", "agent_id": foreign }),
            note: None,
            track: None,
            notify_user: None,
        },
    )
    .expect_err("an agent outside the project");
    assert!(
        refused.contains(&format!("is not in project {project_id}")),
        "{refused}"
    );

    let after = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(after["result"]["issue"]["assignee"], Value::Null);
    assert_eq!(after["result"]["issue"]["status"], "backlog");
}

/// A tool spells the agent's choice `harness`; the wire spells it `provider`.
/// The daemon maps one to the other at the boundary, so a tool naming a
/// harness gets an agent on that harness.
#[test]
fn a_tools_harness_becomes_the_wires_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let target = workspace(&mut state, &project_id, "there");
    let id = filed(&mut state, &project_id, "one")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let handed = call(
        &mut state,
        &who,
        BridgeAction::TrackerAssignIssue {
            issue_id: id,
            assignee: json!({
                "kind": "new_agent",
                "workspace_id": target,
                "harness": "codex",
            }),
            note: None,
            track: None,
            notify_user: None,
        },
    )
    .expect("a tool names a harness");
    let made = handed["dispatch"]["agent_id"].as_str().unwrap().to_string();
    let entity_id = handed["dispatch"]["entity_id"].as_str().unwrap();

    let roster = state.handle(req("agent.list", json!({ "entity_id": entity_id })));
    let agent = roster["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == made.as_str())
        .unwrap_or_else(|| panic!("the made agent is on the roster: {roster:?}"));
    assert_eq!(
        agent["provider"], "codex",
        "the tool's harness is the wire's provider"
    );
}

/// The prompt note is on both working surfaces and on neither of the others,
/// and it says the four things an agent gets wrong without being told.
#[test]
fn the_issue_note_is_on_every_template_that_carries_the_tools() {
    let templates = crate::templates::Templates::default();
    let carries = |text: &str| text.contains("`assign_issue`") && text.contains("In review");

    for (name, text) in [
        ("build", &templates.build),
        ("build_stage", &templates.build_stage),
        ("plan", &templates.plan),
        ("revise", &templates.revise),
        ("revise_stage", &templates.revise_stage),
        ("review_changes", &templates.review_changes),
        ("message", &templates.message),
        ("project_agent", &templates.project_agent),
    ] {
        assert!(carries(text), "{name} does not carry the issue note");
    }
    assert!(
        !carries(&templates.router),
        "the router has no project and no issues"
    );

    // The four things it has to say.
    let note = &templates.build;
    assert!(note.contains("that issue is the work"), "{note}");
    assert!(note.contains("`comment_issue`"), "{note}");
    assert!(
        note.contains("not that it is accepted"),
        "In review is not Done"
    );
    assert!(note.contains("File an issue for follow-up work"), "{note}");
}
