//! What the tracker pushes, and what it does without being asked (spec:
//! Issues → Push, Automatic activity).

use super::project_agent::workspace;
use super::tracker::{filed, tracked, tracked_with_origin};
use super::*;
use crate::mcp::{DoneReport, DoneStatus};

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

/// The event kinds one issue carries, in order.
fn event_kinds(state: &mut AppState, issue_id: &str) -> Vec<String> {
    state.handle(req("issues.get", json!({ "issue_id": issue_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "event")
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The comment bodies one issue carries.
fn comment_bodies(state: &mut AppState, issue_id: &str) -> Vec<String> {
    state.handle(req("issues.get", json!({ "issue_id": issue_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "comment")
        .map(|entry| entry["body"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn report(status: DoneStatus, summary: &str) -> DoneReport {
    DoneReport {
        status,
        summary: summary.to_string(),
        message_id: None,
    }
}

// ------------------------------------------------------------------ push ---

/// A tracker write reaches a session subscribed to its project as a `changes`
/// frame naming the issue that moved — the verb, the bus and the flusher wired
/// end to end.
#[tokio::test]
async fn a_tracker_write_is_pushed_to_a_subscription_on_its_project() {
    let (dir, repo) = init_repo();
    let (_state, handler, sender, mut rx, key) =
        super::push::greeted_push_session(&repo, dir.path());
    let board = handler.call(sender.clone(), req("board.list", json!({})));
    let project_id = board["result"]["projects"][0]["project_id"]
        .as_str()
        .expect("the QA daemon lists its repo as a project")
        .to_string();

    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-inbox",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["issues"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    assert_eq!(
        subscribed["result"]["watch"], "live",
        "issues need no watcher, so a subscription for them is never polled"
    );
    super::push::settled_pushes(&mut rx, &key).await;

    let filed = handler.call(
        sender.clone(),
        req(
            "issues.create",
            json!({ "project_id": project_id, "title": "pushed" }),
        ),
    );
    assert_eq!(filed["ok"], true, "{filed:?}");
    let id = filed["result"]["issue"]["id"].as_str().unwrap().to_string();

    let pushed = super::push::settled_pushes(&mut rx, &key).await;
    let item = pushed
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|push| push["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item.get("issues").is_some())
        .unwrap_or_else(|| panic!("no issues item: {pushed:?}"));
    assert_eq!(
        item["entity_id"],
        project_id.as_str(),
        "an issues item is about a project, not a work item"
    );
    assert_eq!(item["issues"]["issue_ids"], json!([id]));
    assert_eq!(item["issues"]["truncated"], false);

    // And a comment on it pushes too, so a board follows the work.
    let said = handler.call(
        sender.clone(),
        req(
            "issues.comment",
            json!({ "issue_id": id, "body": "a word" }),
        ),
    );
    assert_eq!(said["ok"], true, "{said:?}");
    let after = super::push::settled_pushes(&mut rx, &key).await;
    assert!(
        after
            .iter()
            .filter(|push| push["type"] == "changes")
            .flat_map(|push| push["items"].as_array().cloned().unwrap_or_default())
            .any(|item| item["issues"]["issue_ids"] == json!([id])),
        "{after:?}"
    );
}

// ----------------------------------------------------- automatic activity ---

/// An agent holding a dispatched issue reports Complete: the issue moves to In
/// review, and NOTHING is written on it as a comment.
#[test]
fn a_complete_from_the_agent_holding_an_issue_moves_it_and_says_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));
    let handed = state.handle(req(
        "issues.assign",
        json!({
            "issue_id": id,
            "assignee": { "kind": "new_agent", "workspace_id": ws }
        }),
    ));
    assert_eq!(handed["ok"], true, "{handed:?}");
    let dispatch = &handed["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Fixed the drop handler race."),
    );

    assert_eq!(
        comment_bodies(&mut state, &id),
        Vec::<String>::new(),
        "a report is a message to the user, not a comment on the issue"
    );
    let read = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(
        read["result"]["issue"]["status"], "in_review",
        "Complete means ready to be looked at"
    );
    assert_eq!(
        read["result"]["issue"]["state"], "open",
        "and not accepted — the agent does not close its own issue"
    );
    let moved = state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "moved" && entry["payload"]["by"] == "report")
        .cloned()
        .unwrap_or_else(|| panic!("no move by report"));
    assert_eq!(moved["payload"]["to"], "in_review");
    assert_eq!(
        moved["actor"],
        json!({ "kind": "agent", "agent_id": agent_id })
    );
}

/// An agent whose turn stopped at a usage limit mid-issue (#58) is still working
/// that issue: through the hold, the lift and the resume turn, the marker the
/// dispatch set stays put, so the Complete the resumed turn reports moves the
/// card it was dispatched for. Pinned so a later change to WHEN the marker is
/// taken cannot drop it on the way through a limit.
#[test]
fn a_complete_after_a_usage_limit_still_moves_the_issue_the_turn_was_dispatched_under() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));
    let handed = state.handle(req(
        "issues.assign",
        json!({
            "issue_id": id,
            "assignee": { "kind": "new_agent", "workspace_id": ws }
        }),
    ));
    assert_eq!(handed["ok"], true, "{handed:?}");
    let dispatch = &handed["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    // The dispatch's own turn goes out and runs.
    let mut started = state.take_pending_turns();
    while let Some((_, mark)) = started.next_turn() {
        mark.settle(&mut state);
    }

    // It stops at the limit, then the human retries before the reported reset.
    let resets_at = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let limited = crate::harness::SessionStatusSnapshot::new(crate::harness::AgentStatus::Waiting)
        .limited(crate::harness::usage_limit::UsageLimited {
            said: "You've hit your session limit · resets 6:20pm (America/New_York)".into(),
            resets_at: Some(resets_at),
        });
    let mut recorded = Default::default();
    state.record_usage_limit(&entity_id, &agent_id, &limited, &mut recorded);
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "body": "how is it going?" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let (_, mark) = state
        .take_pending_turns()
        .next_turn()
        .expect("retry goes before reset");
    mark.settle(&mut state);
    assert_eq!(
        state.dispatched_issue.get(&agent_id),
        Some(&id),
        "through the retry"
    );

    // The limit lifts; the turn that resumes it runs.
    state.release_usage_limits_due_at(resets_at);
    let mut resumed = state.take_pending_turns();
    assert!(resumed.next_turn().is_some(), "the resumed turn goes");
    let running = crate::harness::SessionStatusSnapshot::new(crate::harness::AgentStatus::Working)
        .successful_response();
    state.record_usage_limit(&entity_id, &agent_id, &running, &mut recorded);
    assert_eq!(
        state.dispatched_issue.get(&agent_id),
        Some(&id),
        "through the resume turn"
    );

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Fixed the drop handler race."),
    );

    let read = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(
        read["result"]["issue"]["status"], "in_review",
        "the Complete after the limit moves the dispatched issue"
    );
}

/// Blocked leaves the card where it is: blocked is not ready to be looked at,
/// and a board that said it was would waste a reviewer's time. It writes no
/// comment either, so a Blocked report touches the issue not at all.
#[test]
fn a_blocked_report_leaves_the_card_where_it_is_and_says_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    let before = event_kinds(&mut state, &id).len();

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Blocked, "The fixture will not build."),
    );

    assert_eq!(comment_bodies(&mut state, &id), Vec::<String>::new());
    let read = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(
        read["result"]["issue"]["status"], "in_progress",
        "blocked is not ready to be looked at"
    );
    assert_eq!(
        event_kinds(&mut state, &id).len(),
        before,
        "and nothing claims it moved"
    );
}

/// An agent that holds no issue reports as it always has, and nothing is
/// written anywhere.
#[test]
fn an_agent_holding_no_issue_writes_on_none() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "nobody holds this"));

    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Did something unrelated."),
    );

    assert!(
        comment_bodies(&mut state, &id).is_empty(),
        "an unassigned issue hears nothing"
    );
    assert_eq!(event_kinds(&mut state, &id), vec!["created"]);
}

/// Finishing a workspace closes every open issue that links it, and says why.
#[test]
fn done_on_a_workspace_closes_the_issues_that_link_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let elsewhere = workspace(&mut state, &project_id, "elsewhere");

    let worked = issue_id(&filed(&mut state, &project_id, "worked here"));
    state.handle(req(
        "issues.link",
        json!({ "issue_id": worked, "workspace_id": ws }),
    ));
    let other = issue_id(&filed(&mut state, &project_id, "worked elsewhere"));
    state.handle(req(
        "issues.link",
        json!({ "issue_id": other, "workspace_id": elsewhere }),
    ));
    let unlinked = issue_id(&filed(&mut state, &project_id, "nowhere"));

    let finished = state.handle(req("workspace.finish", json!({ "workspace_id": ws })));
    assert_eq!(finished["ok"], true, "{finished:?}");

    let read = |state: &mut AppState, id: &str| {
        state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["issue"].clone()
    };
    let closed = read(&mut state, &worked);
    assert_eq!(closed["state"], "closed", "the workspace's issue closed");
    assert!(closed["closed_at"].is_string());
    assert_eq!(
        closed["status"], "backlog",
        "closing is not the Done column"
    );
    assert_eq!(
        read(&mut state, &other)["state"],
        "open",
        "another workspace's"
    );
    assert_eq!(
        read(&mut state, &unlinked)["state"],
        "open",
        "and an unlinked one"
    );

    let why = state.handle(req("issues.get", json!({ "issue_id": worked })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "closed")
        .cloned()
        .unwrap_or_else(|| panic!("no closed event"));
    assert_eq!(why["payload"]["reason"], "workspace_finished");
    assert_eq!(why["payload"]["workspace_id"], ws.as_str());
}

/// The issue keeps an author's harness and name after Done removes the
/// workspace and the conversation that originally supplied them.
fn assert_historian_identity(identity: &Value, workspace_id: &str, available: bool) {
    assert_eq!(identity["name"], "Historian");
    assert_eq!(identity["ordinal"], 1);
    assert_eq!(identity["provider"], "pi");
    assert_eq!(identity["workspace_id"], workspace_id);
    assert_eq!(identity["workspace_name"], "identity checkout");
    assert_eq!(identity["available"], available);
}

#[test]
fn issue_identity_survives_workspace_finish() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let ws = workspace(&mut state, &project_id, "identity checkout");
    let id = issue_id(&filed(&mut state, &project_id, "identity in history"));
    let linked = state.handle(req(
        "issues.link",
        json!({ "issue_id": id, "workspace_id": ws }),
    ));
    assert_eq!(linked["ok"], true, "{linked:?}");
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    ));
    let entity_id = ensured["result"]["entity_id"].as_str().unwrap().to_string();
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "provider": "pi" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    state
        .set_agent_name(&entity_id, &agent_id, "Historian")
        .unwrap();
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            crate::mcp::BridgeAction::TrackerCommentIssue {
                issue_id: id.clone(),
                body: "The fix is here.".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("agent comments");
    let before = state.handle(req("issues.get", json!({ "issue_id": id })));
    let identity = &before["result"]["issue"]["identities"][&agent_id];
    assert_historian_identity(identity, &ws, true);

    let reassigned = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "user" } }),
    ));
    assert_eq!(reassigned["ok"], true, "{reassigned:?}");
    assert_eq!(
        reassigned["result"]["issue"]["identities"][&agent_id]["name"], "Historian",
        "reassignment retains the earlier author"
    );

    let finished = state.handle(req("workspace.finish", json!({ "workspace_id": ws })));
    assert_eq!(finished["ok"], true, "{finished:?}");
    let after = state.handle(req("issues.get", json!({ "issue_id": id })));
    let identity = &after["result"]["issue"]["identities"][&agent_id];
    assert_historian_identity(identity, &ws, false);
    let listed = state.handle(req("issues.list", json!({ "project_id": project_id })));
    assert_eq!(
        listed["result"]["issues"][0]["identities"][&agent_id],
        *identity
    );
}

/// An issue already closed is left alone by a finish, so its `closed_at` and
/// its reason are not rewritten by a workspace going away later.
#[test]
fn a_finish_does_not_reclose_an_issue_that_was_already_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "closed by hand"));
    state.handle(req(
        "issues.link",
        json!({ "issue_id": id, "workspace_id": ws }),
    ));
    state.handle(req(
        "issues.close",
        json!({ "issue_id": id, "reason": "not doing this" }),
    ));
    let before = event_kinds(&mut state, &id);

    let finished = state.handle(req("workspace.finish", json!({ "workspace_id": ws })));
    assert_eq!(finished["ok"], true, "{finished:?}");

    assert_eq!(
        event_kinds(&mut state, &id),
        before,
        "a closed issue is not closed twice"
    );
    let closed = state.handle(req("issues.get", json!({ "issue_id": id })));
    let reason = closed["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "closed")
        .unwrap()["payload"]["reason"]
        .clone();
    assert_eq!(reason, "not doing this", "the first reason stands");
}

/// Nothing an agent SAYS becomes a comment on an issue — not a report, not a
/// message to another agent, not a message that names the issue, and not one
/// sent while looking at it.
///
/// This is the whole of the rule that #35 exists for. A conversation message
/// is a conversation message; `comment_issue` and `issues.comment` are the two
/// things that write a comment, and they are the only two.
#[test]
fn nothing_an_agent_says_lands_on_the_issue_as_a_comment() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"].clone();
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();

    // A second agent of the project, to be written to.
    let (_owner, colleague) = super::project_agent::project_agent(&mut state, &project_id);

    // 1. A message naming the issue by number, posted into the conversation.
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": entity_id,
            "agent_id": agent_id,
            "body": "#1 Kanban drag is the one I am on; the drop handler races.",
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    // 2. A message sent while looking at that issue.
    let looking = state.handle(req(
        "thread.post",
        json!({
            "entity_id": entity_id,
            "agent_id": agent_id,
            "body": "is this one done?",
            "viewing_context": {
                "version": 1,
                "items": [{
                    "kind": "issue",
                    "issue_id": id,
                    "number": 1,
                    "title": "Kanban drag"
                }]
            },
        }),
    ));
    assert_eq!(looking["ok"], true, "{looking:?}");

    // 3. The agent's outgoing message to another agent — the case seen on #32,
    //    where a report sent on with `message_agent` appeared as a comment.
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            crate::mcp::BridgeAction::MessageAgent {
                agent_id: colleague.clone(),
                body: "Fixed the drop handler race on #1; nothing is blocked on me.".into(),
            },
        )
        .expect("an agent may write to a colleague");

    // 4. And its own end-of-turn report.
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Fixed the drop handler race."),
    );

    assert_eq!(
        comment_bodies(&mut state, &id),
        Vec::<String>::new(),
        "only comment_issue writes a comment"
    );

    // And the one thing that DOES write one still does.
    state
        .on_agent_mcp_action(
            &entity_id,
            &agent_id,
            crate::mcp::BridgeAction::TrackerCommentIssue {
                issue_id: id.clone(),
                body: "Said deliberately.".into(),
                refs: Vec::new(),
                track: None,
                attachments: Vec::new(),
                notify_user: None,
                mention_user: None,
            },
        )
        .expect("comment_issue writes a comment");
    assert_eq!(
        comment_bodies(&mut state, &id),
        vec!["Said deliberately.".to_string()]
    );
}

/// A Complete moves the issue the turn was dispatched under, and no other.
///
/// #48: an agent commonly holds a queue. The transport agent was assigned #30,
/// #31 and #41, worked the first two, and its report moved #41 — which nobody
/// had touched — to In review. It had to be moved back by hand.
#[test]
fn a_complete_moves_only_the_issue_the_turn_was_dispatched_under() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let worked = issue_id(&filed(&mut state, &project_id, "the one it was given"));
    let untouched = issue_id(&filed(&mut state, &project_id, "still in its queue"));

    // Dispatched the first, which makes the agent.
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": worked, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"].clone();
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();

    // And the second, which queues behind it. This is the last dispatch, so
    // the turn now running is the one it started.
    state.handle(req(
        "issues.assign",
        json!({ "issue_id": untouched, "assignee": { "kind": "agent", "agent_id": agent_id } }),
    ));

    let column = |state: &mut AppState, id: &str| {
        state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["issue"]["status"]
            .as_str()
            .unwrap()
            .to_string()
    };

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Did the second one."),
    );

    assert_eq!(
        column(&mut state, &untouched),
        "in_review",
        "the issue this turn was dispatched under"
    );
    assert_eq!(
        column(&mut state, &worked),
        "in_progress",
        "and the one still in its queue is left exactly where it was"
    );

    // A second Complete in the same turn moves nothing more: the marker
    // belonged to that turn and the report was the end of it.
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Still done."),
    );
    assert_eq!(column(&mut state, &worked), "in_progress");
}

/// A turn nobody dispatched moves nothing. A reviewer message, a notice, a
/// restart: a report on one of those says nothing about any issue.
#[test]
fn a_report_on_a_turn_no_assignment_started_moves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(
        &mut state,
        &project_id,
        "assigned, not dispatched into this turn",
    ));
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"].clone();
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();

    // The dispatch's own turn is reported and the card moves.
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "Done."),
    );
    let read = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(read["result"]["issue"]["status"], "in_review");

    // The user then says something and the agent answers. It still HOLDS the
    // issue, but this turn was not about it.
    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "in_progress" }),
    ));
    state.handle(req(
        "thread.post",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "body": "and what about the rail?" }),
    ));
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        report(DoneStatus::Completed, "The rail is fine."),
    );

    let read = state.handle(req("issues.get", json!({ "issue_id": id })));
    assert_eq!(
        read["result"]["issue"]["status"], "in_progress",
        "no assignment started that turn, so nothing moved: {read:?}"
    );
}
