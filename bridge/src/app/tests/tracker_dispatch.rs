//! Assigning an issue delivers it and starts the agent (spec: Issues →
//! Assignment is dispatch).
//!
//! Each of the five kinds, the refusal for an agent outside the project, and
//! the envelope the delivered message carries.

use super::project_agent::{project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

fn assign(state: &mut AppState, issue_id: &str, assignee: Value) -> Value {
    let answered = state.handle(req(
        "issues.assign",
        json!({ "issue_id": issue_id, "assignee": assignee }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    answered["result"].clone()
}

fn refusal(state: &mut AppState, issue_id: &str, assignee: Value) -> String {
    let answered = state.handle(req(
        "issues.assign",
        json!({ "issue_id": issue_id, "assignee": assignee }),
    ));
    assert_eq!(answered["ok"], false, "{answered:?}");
    answered["error"].as_str().unwrap_or_default().to_string()
}

/// The messages on one conversation, newest last.
fn messages(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
    let read = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "limit": 50 }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    read["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .collect()
}

/// The timeline event kinds one issue carries, in order.
fn event_kinds(state: &mut AppState, issue_id: &str) -> Vec<String> {
    state.handle(req("issues.get", json!({ "issue_id": issue_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "event")
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assigning to the user holds the issue and starts nobody.
#[test]
fn assigning_to_the_user_dispatches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let answered = assign(&mut state, &id, json!({ "kind": "user" }));
    assert_eq!(answered["issue"]["assignee"], json!({ "kind": "user" }));
    assert_eq!(answered["dispatch"], Value::Null, "nothing was started");
    assert_eq!(
        answered["issue"]["status"], "backlog",
        "nothing started, so nothing moved"
    );
    assert_eq!(event_kinds(&mut state, &id), vec!["created", "assigned"]);
}

/// Unassigning is a legible thing to ask for, writes `unassigned`, and stops
/// nothing that is already running.
#[test]
fn unassigning_says_so_and_stops_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    assign(&mut state, &id, json!({ "kind": "user" }));

    let answered = assign(&mut state, &id, Value::Null);
    assert_eq!(answered["issue"]["assignee"], Value::Null);
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "assigned", "unassigned"]
    );
}

/// Assign to the project's agent and answer what the dispatch said. The one
/// setup two tests share, so neither repeats it.
fn handed_to_the_project_agent(state: &mut AppState, project_id: &str) -> (String, Value) {
    let id = issue_id(&filed(state, project_id, "Kanban drag does not persist"));
    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "body": "Dragging a card puts it back." }),
    ));
    let answered = assign(state, &id, json!({ "kind": "project_agent" }));
    (id, answered)
}

/// Assigning to the project's agent ensures the conversation, records where
/// the work went, and moves the card.
#[test]
fn assigning_to_the_project_agent_records_where_the_work_went() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (id, answered) = handed_to_the_project_agent(&mut state, &project_id);

    assert_eq!(
        answered["issue"]["assignee"],
        json!({ "kind": "project_agent" })
    );
    let dispatch = &answered["dispatch"];
    assert_eq!(dispatch["kind"], "project_agent");
    assert_eq!(
        dispatch["workspace_id"],
        Value::Null,
        "it holds no checkout"
    );
    assert!(
        crate::agent::is_project_agent(dispatch["agent_id"].as_str().unwrap()),
        "the project's conversation mints a project agent: {dispatch:?}"
    );
    assert!(dispatch["operation_id"]
        .as_str()
        .unwrap()
        .starts_with("op-"));
    assert_eq!(
        answered["issue"]["links"]["conversation_ids"],
        json!([dispatch["entity_id"].as_str().unwrap()]),
        "the issue records where the work went"
    );
    assert_eq!(
        answered["issue"]["status"], "in_progress",
        "starting work moves the card"
    );
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "assigned", "tracked", "dispatched", "moved"]
    );
}

/// An assignment is a NOTICE, not the issue. The body names who assigned what,
/// and the envelope identifies it for a client's card — the issue text itself
/// is never copied into the conversation, so it cannot be sitting there going
/// stale, or being compacted away, before the work begins.
#[test]
fn the_delivered_message_is_a_notice_naming_the_issue_and_not_a_copy_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (id, answered) = handed_to_the_project_agent(&mut state, &project_id);
    let dispatch = &answered["dispatch"].clone();

    let delivered = messages(
        &mut state,
        dispatch["entity_id"].as_str().unwrap(),
        dispatch["agent_id"].as_str().unwrap(),
    );
    let handed = delivered
        .iter()
        .find(|message| message["from_issue"].is_object())
        .unwrap_or_else(|| panic!("no message wearing the issue: {delivered:?}"));

    assert_eq!(handed["from_issue"]["issue_id"], id.as_str());
    assert_eq!(handed["from_issue"]["number"], 1);
    assert_eq!(
        handed["from_issue"]["title"],
        "Kanban drag does not persist"
    );
    assert!(
        handed["from_issue"]["body"].is_null(),
        "the envelope identifies the issue, it does not carry it: {handed:?}"
    );
    assert!(handed["from_issue"]["links"].is_object());
    assert_eq!(
        handed["role"], "user",
        "an instruction arrives on the user's side whoever wrote it"
    );
    assert!(
        handed["from_agent"].is_null(),
        "the human assigned it, so nobody signed it: {handed:?}"
    );
    assert_eq!(
        handed["body"], "The user assigned you issue #1 — Kanban drag does not persist",
        "one line, and the issue's own words are not in it"
    );
    assert!(
        !handed["body"]
            .as_str()
            .unwrap()
            .contains("Dragging a card puts it back."),
        "the issue body is read with get_issue, not delivered: {handed:?}"
    );
}

/// An agent that assigns is named the way the conversation names it, so the
/// reader knows which of its colleagues is asking rather than being handed an
/// id to go and look up.
#[test]
fn an_agent_that_assigns_is_named_in_the_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));
    let ws = workspace(&mut state, &project_id, "wire-facade");
    let second = issue_id(&filed(&mut state, &project_id, "two"));
    let made = assign(
        &mut state,
        &second,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    let assigner = made["dispatch"]["agent_id"].as_str().unwrap().to_string();
    let (owner, project_agent_id) = project_agent(&mut state, &project_id);

    let from = made["dispatch"]["entity_id"].as_str().unwrap().to_string();
    state
        .on_agent_mcp_action(
            &from,
            &assigner,
            crate::mcp::BridgeAction::TrackerAssignIssue {
                issue_id: id.clone(),
                assignee: json!({ "kind": "project_agent" }),
                note: None,
            },
        )
        .expect("an agent may assign");

    let delivered = messages(&mut state, &owner, &project_agent_id);
    let handed = delivered
        .iter()
        .find(|message| message["from_issue"].is_object())
        .unwrap_or_else(|| panic!("no message wearing the issue: {delivered:?}"));
    assert_eq!(
        handed["body"], "The wire-facade agent assigned you issue #1 — Kanban drag",
        "named by the workspace it works in: {handed:?}"
    );
}

/// A note rides under the issue in the delivered message and is not stored on
/// the issue: the body is the issue.
#[test]
fn a_hand_off_note_is_delivered_and_not_written_onto_the_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let answered = state.handle(req(
        "issues.assign",
        json!({
            "issue_id": id,
            "assignee": { "kind": "project_agent" },
            "note": "Start with the drop handler."
        }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(
        answered["result"]["issue"]["body"], "",
        "the note is not the issue"
    );
    let dispatch = &answered["result"]["dispatch"];
    let delivered = messages(
        &mut state,
        dispatch["entity_id"].as_str().unwrap(),
        dispatch["agent_id"].as_str().unwrap(),
    );
    let handed = delivered
        .iter()
        .find(|message| message["from_issue"].is_object())
        .unwrap_or_else(|| panic!("no message wearing the issue: {delivered:?}"));
    assert_eq!(
        handed["body"], "The user assigned you issue #1 — one\n\nStart with the drop handler.",
        "the note is what the notice has to say beyond naming the issue"
    );
}

/// `new_agent` puts an agent on a workspace of this project and hands it the
/// issue; the issue records both the workspace and the conversation.
#[test]
fn a_new_agent_on_an_existing_workspace_is_made_and_handed_the_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");

    let answered = assign(
        &mut state,
        &id,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    let dispatch = &answered["dispatch"];
    assert_eq!(dispatch["kind"], "new_agent");
    assert_eq!(dispatch["workspace_id"], ws);
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    assert_eq!(
        answered["issue"]["assignee"],
        json!({ "kind": "agent", "agent_id": agent_id }),
        "a creating kind resolves to the agent it made"
    );
    assert_eq!(
        answered["issue"]["links"]["conversation_ids"],
        json!([dispatch["entity_id"].as_str().unwrap()])
    );
    assert_eq!(answered["issue"]["links"]["workspace_ids"], json!([ws]));
    assert_eq!(answered["issue"]["status"], "in_progress");

    let delivered = messages(
        &mut state,
        dispatch["entity_id"].as_str().unwrap(),
        &agent_id,
    );
    assert!(
        delivered
            .iter()
            .any(|message| message["from_issue"]["issue_id"] == id.as_str()),
        "{delivered:?}"
    );
}

/// An agent of this project can be handed an issue by id; one outside it is
/// refused in the same words every project-scoped handler uses.
#[test]
fn an_agent_of_another_project_is_refused_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");
    let here = assign(
        &mut state,
        &id,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    let mine = here["dispatch"]["agent_id"].as_str().unwrap().to_string();

    // An agent of a second project on the same device.
    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = super::project_agent::added_project(&mut state, &other_repo);
    let (_owner, foreign) = project_agent(&mut state, &other_project);

    let refused = refusal(
        &mut state,
        &id,
        json!({ "kind": "agent", "agent_id": foreign }),
    );
    assert!(
        refused.contains(&format!("is not in project {project_id}")),
        "{refused}"
    );
    assert!(
        refusal(
            &mut state,
            &id,
            json!({ "kind": "agent", "agent_id": "agent-nobody" })
        )
        .contains("unknown agent_id"),
        "an id nobody answers to is named too"
    );

    // And the refusal left the issue exactly as it was.
    let after = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(
        after["result"]["issue"]["assignee"],
        json!({ "kind": "agent", "agent_id": mine }),
        "a refused assignment changes nothing"
    );

    // Re-assigning to an agent of THIS project by id is accepted.
    let reassigned = assign(
        &mut state,
        &id,
        json!({ "kind": "agent", "agent_id": mine }),
    );
    assert_eq!(reassigned["dispatch"]["kind"], "agent");
}

/// A dispatch moves the card off the columns that mean "not started" and
/// leaves one already further along where it was put.
#[test]
fn a_dispatch_moves_a_card_off_backlog_and_never_rewinds_one() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");

    let ready = issue_id(&filed(&mut state, &project_id, "ready"));
    state.handle(req(
        "issues.update",
        json!({ "issue_id": ready, "status": "ready" }),
    ));
    let moved = assign(
        &mut state,
        &ready,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    assert_eq!(moved["issue"]["status"], "in_progress");

    let reviewing = issue_id(&filed(&mut state, &project_id, "reviewing"));
    state.handle(req(
        "issues.update",
        json!({ "issue_id": reviewing, "status": "in_review" }),
    ));
    // The update itself moved it, so what the dispatch added is what matters.
    let before = event_kinds(&mut state, &reviewing).len();
    let held = assign(
        &mut state,
        &reviewing,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    assert_eq!(
        held["issue"]["status"], "in_review",
        "a board position set deliberately is not rewound by a reassignment"
    );
    let added = event_kinds(&mut state, &reviewing).split_off(before);
    assert_eq!(
        added,
        vec![
            "assigned".to_string(),
            // Assignment subscribes the agent it hands the work to, which is
            // a consequence of the assignment and so reads after it.
            "tracked".to_string(),
            "dispatched".to_string(),
        ],
        "the dispatch says what it did and claims no move it did not make"
    );
}

/// An assignee this bridge does not know is refused by name, naming the five
/// kinds there are.
#[test]
fn an_assignee_kind_this_bridge_does_not_know_names_the_ones_it_does() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let refused = refusal(&mut state, &id, json!({ "kind": "the_intern" }));
    assert!(refused.contains("new_workspace"), "{refused}");
    assert!(refused.contains("project_agent"), "{refused}");

    let answered = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "the_intern" } }),
    ));
    assert_eq!(answered["error_code"], "invalid_params", "{answered:?}");

    assert!(
        refusal(&mut state, &id, json!({ "kind": "agent" })).contains("name a agent_id"),
        "a kind that needs an id says which"
    );
    assert!(
        refusal(
            &mut state,
            &id,
            json!({ "kind": "new_agent", "workspace_id": "ws-nobody" })
        )
        .contains("unknown workspace_id"),
        "a workspace nobody answers to is named"
    );
}

/// `new_workspace` cuts a checkout, puts an agent in it, and hands it the
/// issue — all of it off the app mutex, answered from the drain.
#[test]
fn a_new_workspace_is_cut_an_agent_added_and_the_issue_handed_over() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));

    let answered = assign(
        &mut state,
        &id,
        json!({ "kind": "new_workspace", "isolation": "worktree" }),
    );
    let dispatch = &answered["dispatch"];
    assert_eq!(dispatch["kind"], "new_workspace");
    let workspace_id = dispatch["workspace_id"].as_str().unwrap().to_string();
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    assert!(
        dispatch["operation_id"]
            .as_str()
            .unwrap()
            .starts_with("op-"),
        "the receipt is the DELIVERY's, not the workspace cut's: {dispatch:?}"
    );

    // The workspace is a real one of this project, named after the issue.
    let listed = state.handle(req("workspace.list", json!({ "project_id": project_id })));
    let workspace = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["workspace_id"] == workspace_id.as_str())
        .unwrap_or_else(|| panic!("the cut workspace is not listed: {listed:?}"));
    assert_eq!(workspace["status"], "ready");
    assert_eq!(
        workspace["name"], "Kanban drag",
        "a workspace cut for an issue is named after it"
    );

    assert_eq!(
        answered["issue"]["assignee"],
        json!({ "kind": "agent", "agent_id": agent_id })
    );
    assert_eq!(
        answered["issue"]["links"]["workspace_ids"],
        json!([workspace_id]),
        "the issue records what was made for it"
    );
    assert_eq!(
        answered["issue"]["links"]["conversation_ids"],
        json!([entity_id])
    );
    assert_eq!(answered["issue"]["status"], "in_progress");
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "assigned", "tracked", "dispatched", "moved"]
    );

    let delivered = messages(&mut state, &entity_id, &agent_id);
    assert!(
        delivered
            .iter()
            .any(|message| message["from_issue"]["issue_id"] == id.as_str()),
        "the issue was handed over: {delivered:?}"
    );
}

/// A `new_workspace` dispatch that cannot cut its checkout leaves the issue
/// untouched: nothing may be assigned to an agent that was never made.
#[test]
fn a_workspace_that_cannot_be_cut_leaves_the_issue_unassigned() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let answered = state.handle(req(
        "issues.assign",
        json!({
            "issue_id": id,
            "assignee": { "kind": "new_workspace", "isolation": "hovercraft" }
        }),
    ));
    assert_eq!(answered["ok"], false, "{answered:?}");
    assert!(
        answered["error"]
            .as_str()
            .unwrap()
            .contains("unknown isolation"),
        "{answered:?}"
    );

    let after = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(after["result"]["issue"]["assignee"], Value::Null);
    assert_eq!(after["result"]["issue"]["status"], "backlog");
    assert_eq!(event_kinds(&mut state, &id), vec!["created"]);
}

/// Handing an issue to an agent that already exists records the workspace that
/// agent is working in, not just its conversation.
///
/// This is what makes self-assignment worth asking an agent for: an agent that
/// assigns itself the issue it is working on links the checkout by doing it,
/// and nobody has to remember a second call.
#[test]
fn assigning_to_an_existing_agent_links_the_workspace_it_works_in() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");

    // An agent on that workspace, made without any issue in hand.
    let second = issue_id(&filed(&mut state, &project_id, "two"));
    let made = assign(
        &mut state,
        &second,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    let agent_id = made["dispatch"]["agent_id"].as_str().unwrap().to_string();

    let answered = assign(
        &mut state,
        &id,
        json!({ "kind": "agent", "agent_id": agent_id }),
    );
    assert_eq!(
        answered["dispatch"]["workspace_id"], ws,
        "the dispatch says where the work is: {answered:?}"
    );
    assert_eq!(
        answered["issue"]["links"]["workspace_ids"],
        json!([ws]),
        "and the issue records it"
    );
    assert_eq!(
        answered["issue"]["links"]["conversation_ids"],
        json!([answered["dispatch"]["entity_id"].as_str().unwrap()])
    );
}

/// Unassigning leaves the links alone. What the issue was worked in is a fact
/// about its history; handing it back does not unmake the checkout, and an
/// issue that forgot where the work happened would be worse off than one
/// nobody holds.
#[test]
fn unassigning_does_not_unlink_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");
    let made = assign(
        &mut state,
        &id,
        json!({ "kind": "new_agent", "workspace_id": ws }),
    );
    let linked = made["issue"]["links"].clone();
    assert_eq!(linked["workspace_ids"], json!([ws]));

    let after = assign(&mut state, &id, Value::Null);
    assert_eq!(after["issue"]["assignee"], Value::Null);
    assert_eq!(
        after["issue"]["links"], linked,
        "handing it back forgets nothing"
    );
}

/// The prompt says what an assignment IS now, because the notice no longer
/// carries the issue and an agent that does not call `get_issue` would start
/// on a title alone.
#[test]
fn the_prompt_says_an_assignment_is_a_notice_to_be_read_with_get_issue() {
    let templates = crate::templates::Templates::default();
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, text) in [
        ("build", &templates.build),
        ("plan", &templates.plan),
        ("project_agent", &templates.project_agent),
    ] {
        let text = flat(text);
        assert!(
            text.contains("The message is only a notice naming it"),
            "{name} does not say what arrives"
        );
        assert!(
            text.contains("read it with `get_issue` before you start"),
            "{name} does not say to read it"
        );
        assert!(
            text.contains("again if you have been running a while"),
            "{name} does not say to re-read a stale one"
        );
    }
}

/// The prompt asks for the self-assignment that does the linking. What
/// `link_issue` is left saying for itself is pinned in `mcp.rs`.
#[test]
fn the_prompt_asks_an_agent_to_assign_itself_what_it_is_working_on() {
    let templates = crate::templates::Templates::default();
    // Collapsed, so the assertions read the wording rather than the wrapping.
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, text) in [
        ("build", &templates.build),
        ("plan", &templates.plan),
        ("project_agent", &templates.project_agent),
    ] {
        let text = flat(text);
        assert!(
            text.contains("Assign yourself any issue you pick up that nobody handed you"),
            "{name} does not ask for it"
        );
        assert!(
            text.contains("assigning records the workspace and conversation"),
            "{name} does not say what assigning yourself records"
        );
    }
}
