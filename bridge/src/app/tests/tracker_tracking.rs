//! Who is watching an issue, and what one agent is on (spec: Issues →
//! Tracking).

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;
use crate::mcp::{DoneReport, DoneStatus};

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

/// A coding agent on a workspace of this project, as `(entity_id, agent_id)`.
fn coding_agent(state: &mut AppState, project_id: &str, name: &str) -> (String, String) {
    let workspace_id = workspace(state, project_id, name);
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ));
    let entity_id = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = state.handle(req("agent.add", json!({ "entity_id": entity_id })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    (entity_id, agent_id)
}

fn trackers(state: &mut AppState, issue_id: &str) -> Vec<String> {
    state.handle(req("issues.get", json!({ "issue_id": issue_id })))["result"]["issue"]["trackers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

fn event_kinds(state: &mut AppState, issue_id: &str) -> Vec<String> {
    state.handle(req("issues.get", json!({ "issue_id": issue_id })))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["type"] == "event")
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Tracking and untracking move the list and say so on the timeline.
#[test]
fn tracking_adds_the_agent_and_untracking_takes_it_off() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_entity, watcher) = coding_agent(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    assert_eq!(
        trackers(&mut state, &id),
        Vec::<String>::new(),
        "nobody yet"
    );

    let tracking = state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    assert_eq!(tracking["ok"], true, "{tracking:?}");
    assert_eq!(tracking["result"]["issue"]["trackers"], json!([watcher]));
    assert_eq!(event_kinds(&mut state, &id), vec!["created", "tracked"]);

    let untracking = state.handle(req(
        "issues.untrack",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    assert_eq!(untracking["ok"], true, "{untracking:?}");
    assert_eq!(untracking["result"]["issue"]["trackers"], json!([]));
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "tracked", "untracked"]
    );
}

/// Saying it twice is not a second fact: the list and the timeline both stay
/// where they were.
#[test]
fn tracking_twice_and_untracking_nothing_write_no_event() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_entity, watcher) = coding_agent(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    // Untracking one that was never tracked changes nothing.
    let never = state.handle(req(
        "issues.untrack",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    assert_eq!(never["ok"], true, "a no-op is legible, not a refusal");
    assert_eq!(event_kinds(&mut state, &id), vec!["created"]);

    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher }),
    ));
    assert_eq!(trackers(&mut state, &id), vec![watcher], "one entry");
    assert_eq!(
        event_kinds(&mut state, &id),
        vec!["created", "tracked"],
        "and one event"
    );
}

/// The agent an issue is assigned to is subscribed by the assignment, and the
/// event says the tracking was a consequence rather than a request.
#[test]
fn assignment_tracks_the_agent_it_hands_the_work_to() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "hand this over"));

    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    assert_eq!(handed["ok"], true, "{handed:?}");
    let assignee = handed["result"]["dispatch"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        handed["result"]["issue"]["trackers"],
        json!([assignee]),
        "the agent that gets the work hears about the issue"
    );

    let tracked_event = state.handle(req("issues.get", json!({ "issue_id": id })))["result"]
        ["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == "tracked")
        .cloned()
        .expect("a tracked event");
    assert_eq!(tracked_event["payload"]["agent_id"], assignee.as_str());
    assert_eq!(
        tracked_event["payload"]["by"], "assignment",
        "a consequence, not a request"
    );

    // And unassigning leaves it watching: handing work on is exactly when the
    // previous holder still wants to know how it went.
    let unassigned = state.handle(req(
        "issues.assign",
        json!({ "issue_id": id, "assignee": Value::Null }),
    ));
    assert_eq!(unassigned["ok"], true, "{unassigned:?}");
    assert_eq!(
        unassigned["result"]["issue"]["trackers"],
        json!([assignee]),
        "unassigning does not untrack"
    );
}

/// An agent of another project cannot be made to watch, and the issue is left
/// as it was.
#[test]
fn an_agent_outside_the_issues_project_cannot_be_made_to_watch() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    let (_owner, foreign) = project_agent(&mut state, &other_project);

    let refused = state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": foreign }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains(&format!("is not in project {project_id}")),
        "{refused:?}"
    );
    assert_eq!(trackers(&mut state, &id), Vec::<String>::new());

    let nobody = state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": "agent-nobody" }),
    ));
    assert!(
        nobody["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent_id"),
        "{nobody:?}"
    );
}

/// `issues.for_agent` answers digests in two lists, newest-updated first, and
/// an assigned issue is in both.
#[test]
fn the_per_agent_read_answers_what_it_holds_and_what_it_watches() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let (_entity, other) = coding_agent(&mut state, &project_id, "elsewhere");

    let held = issue_id(&filed(&mut state, &project_id, "held"));
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": held, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let holder = handed["result"]["dispatch"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();

    let watched = issue_id(&filed(&mut state, &project_id, "watched"));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": watched, "agent_id": holder }),
    ));
    filed(&mut state, &project_id, "neither");

    let read = state.handle(req("issues.for_agent", json!({ "agent_id": holder })));
    assert_eq!(read["ok"], true, "{read:?}");
    let ids = |key: &str| -> Vec<String> {
        read["result"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["issue_id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids("assigned"), vec![held.clone()]);
    assert_eq!(
        ids("tracking"),
        vec![watched.clone(), held.clone()],
        "assignment tracks too, so the held issue is in BOTH lists"
    );

    // A digest is enough to recognise and to order by, and not the body.
    let entry = &read["result"]["assigned"][0];
    assert_eq!(entry["number"], 1);
    assert_eq!(entry["title"], "held");
    assert_eq!(entry["state"], "open");
    assert!(entry["status"].is_string());
    assert!(entry["updated_at"].is_string());
    assert!(entry.get("body").is_none(), "a digest carries no body");

    // Another agent of the same project holds and watches nothing.
    let empty = state.handle(req("issues.for_agent", json!({ "agent_id": other })));
    assert_eq!(empty["result"]["assigned"], json!([]));
    assert_eq!(empty["result"]["tracking"], json!([]));
}

// ------------------------------------------------------------- notices ---

/// The notices sitting on one agent's conversation, newest last.
fn notices(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": entity_id, "agent_id": agent_id, "limit": 50 }),
    ));
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .filter(|message| message["from_build"] == true)
        .collect()
}

/// A change reaches every tracker and never the agent that made it.
#[test]
fn a_change_reaches_every_tracker_but_the_agent_that_made_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let actor = coding_agent(&mut state, &project_id, "actor");
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));

    for who in [&actor, &watcher] {
        state.handle(req(
            "issues.track",
            json!({ "issue_id": id, "agent_id": who.1 }),
        ));
    }

    // The actor moves it through its own tool, so the bridge knows who acted.
    state
        .on_agent_mcp_action(
            &actor.0,
            &actor.1,
            crate::mcp::BridgeAction::TrackerMoveIssue {
                issue_id: id.clone(),
                status: "in_review".into(),
            },
        )
        .expect("the actor moves its issue");

    let told = notices(&mut state, &watcher.0, &watcher.1);
    assert_eq!(told.len(), 1, "one notice per change: {told:?}");
    let notice = &told[0];
    assert_eq!(notice["from_build"], true, "Build's own words");
    assert_eq!(notice["role"], "user", "an instruction arrives inbound");
    assert_eq!(notice["from_issue"]["issue_id"], id.as_str());
    assert_eq!(notice["from_issue"]["number"], 1);
    assert_eq!(notice["from_issue"]["title"], "Kanban drag");
    let body = notice["body"].as_str().unwrap();
    assert!(body.contains("moved to In review"), "{body}");
    assert!(body.contains(&actor.1), "it says who: {body}");

    assert!(
        notices(&mut state, &actor.0, &actor.1).is_empty(),
        "nobody is told what they just did"
    );
}

/// A comment's body rides the notice — the point of hearing about a comment is
/// reading it.
#[test]
fn a_comments_body_rides_the_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher.1 }),
    ));

    state.handle(req(
        "issues.comment",
        json!({ "issue_id": id, "body": "The drop handler races the column read." }),
    ));

    let told = notices(&mut state, &watcher.0, &watcher.1);
    assert_eq!(told.len(), 1, "{told:?}");
    let body = told[0]["body"].as_str().unwrap();
    assert!(body.contains("commented"), "{body}");
    assert!(
        body.contains("The drop handler races the column read."),
        "the words themselves: {body}"
    );
}

/// The notice starts the tracker's turn, so an idle agent wakes to it.
#[test]
fn a_notice_starts_the_tracking_agents_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher.1 }),
    ));
    // Nothing queued yet: the tracking call itself woke nobody.
    assert!(state.delivery_queue.take_ready(|_| false).is_empty());

    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "ready" }),
    ));

    let queued = state.delivery_queue.take_ready(|_| false);
    assert_eq!(queued.len(), 1, "one turn for one change");
    let turn = &queued[0];
    assert_eq!(turn.agent_id, watcher.1, "the tracker's turn, not anyone's");
    assert_eq!(turn.owner, watcher.0);
    assert_eq!(turn.phase, "issue_notice");
    assert!(
        turn.says_something(),
        "it tells the agent to go and read, which is what starts an idle one"
    );
    assert!(
        !turn.interrupt,
        "a notice does not cut a turn in flight short"
    );
}

/// A write that changes nothing delivers nothing: it is not news, for the same
/// reason it writes no event.
#[test]
fn a_change_that_changes_nothing_wakes_nobody() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let watcher = coding_agent(&mut state, &project_id, "watcher");
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": watcher.1 }),
    ));

    // It is already in Backlog.
    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "backlog" }),
    ));
    assert!(
        notices(&mut state, &watcher.0, &watcher.1).is_empty(),
        "moving a card where it already is is not news"
    );

    // And somebody else starting to watch is not a change to the issue.
    let other = coding_agent(&mut state, &project_id, "other");
    state.handle(req(
        "issues.track",
        json!({ "issue_id": id, "agent_id": other.1 }),
    ));
    assert!(
        notices(&mut state, &watcher.0, &watcher.1).is_empty(),
        "who else is watching is not a change to the issue"
    );
}

// --------------------------------------------------------------- tools ---

/// The tool subscribes the CALLER. There is no agent id on it to get wrong,
/// and an agent cannot decide what another agent is woken for.
#[test]
fn the_track_tool_subscribes_whoever_called_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let tracked_by_tool = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerTrackIssue {
                issue_id: id.clone(),
            },
        )
        .expect("an agent tracks an issue of its own project");
    assert_eq!(
        tracked_by_tool["issue"]["trackers"],
        json!([caller.1]),
        "the caller, and nobody it might have named"
    );

    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerUntrackIssue {
                issue_id: id.clone(),
            },
        )
        .expect("and stops");
    assert_eq!(trackers(&mut state, &id), Vec::<String>::new());
}

/// An issue of another project is unknown to the tool, the way every other
/// tracker tool reads it.
#[test]
fn the_track_tool_refuses_an_issue_outside_the_callers_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");

    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other_project = added_project(&mut state, &other_repo);
    let theirs = issue_id(&filed(&mut state, &other_project, "not yours"));

    let refused = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerTrackIssue {
                issue_id: theirs.clone(),
            },
        )
        .expect_err("another project's issue");
    assert!(refused.contains("unknown issue_id"), "{refused}");
    assert_eq!(trackers(&mut state, &theirs), Vec::<String>::new());
}

/// The prompt tells an agent the tool exists and the two things it would
/// otherwise get wrong: assignment tracks for you, and unassignment does not
/// untrack.
#[test]
fn the_prompt_says_what_tracking_does_and_does_not_do() {
    let templates = crate::templates::Templates::default();
    // Collapsed, so the assertions read the wording rather than the wrapping.
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (name, text) in [
        ("build", &templates.build),
        ("plan", &templates.plan),
        ("project_agent", &templates.project_agent),
    ] {
        let text = flat(text);
        assert!(text.contains("`track_issue`"), "{name} does not offer it");
        assert!(
            text.contains("tracked automatically on anything assigned to you"),
            "{name} does not say assignment tracks for you"
        );
        assert!(
            text.contains("being unassigned does not"),
            "{name} does not say unassignment leaves you watching"
        );
    }
}

// ------------------------------------------------- the Complete reminder ---

/// A reminder is a `from_build` message naming what is still open.
fn reminders(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<String> {
    notices(state, entity_id, agent_id)
        .into_iter()
        .map(|message| message["body"].as_str().unwrap_or_default().to_string())
        .filter(|body| body.starts_with("You reported Complete"))
        .collect()
}

/// An agent, holding `titles`, that has just reported `status`.
fn reported(
    state: &mut AppState,
    project_id: &str,
    status: DoneStatus,
    holding: &[(&str, &str)],
) -> (String, String) {
    let ws = workspace(state, project_id, "here");
    let first = issue_id(&filed(state, project_id, holding[0].0));
    let handed = state.handle(req(
        "issues.assign",
        json!({ "issue_id": first, "assignee": { "kind": "new_agent", "workspace_id": ws } }),
    ));
    let dispatch = &handed["result"]["dispatch"];
    let entity_id = dispatch["entity_id"].as_str().unwrap().to_string();
    let agent_id = dispatch["agent_id"].as_str().unwrap().to_string();
    state.handle(req(
        "issues.update",
        json!({ "issue_id": first, "status": holding[0].1 }),
    ));
    for (title, column) in &holding[1..] {
        let id = issue_id(&filed(state, project_id, title));
        state.handle(req(
            "issues.assign",
            json!({ "issue_id": id, "assignee": { "kind": "agent", "agent_id": agent_id } }),
        ));
        state.handle(req(
            "issues.update",
            json!({ "issue_id": id, "status": column }),
        ));
    }
    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status,
            summary: "Did the thing.".into(),
            message_id: None,
        },
    );
    (entity_id, agent_id)
}

/// Complete with open assigned issues delivers the list, and names every one.
#[test]
fn complete_with_open_issues_lists_every_one_of_them() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("first", "in_progress"), ("second", "ready")],
    );

    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 1, "one reminder per Complete: {told:?}");
    let body = &told[0];
    assert!(body.contains("#1 first"), "{body}");
    assert!(body.contains("#2 second"), "{body}");
    assert!(
        body.contains("(In progress)"),
        "it says which column: {body}"
    );
    // The reminder runs AFTER the report's own automatic move, so the issue
    // this very Complete pushed to In review is described as it now stands
    // rather than as it stood a moment ago.
    assert!(
        body.contains("#2 second (In review)"),
        "the report moved it, and the reminder says where it is now: {body}"
    );
}

/// Blocked says nothing: the agent has already told us it cannot finish.
#[test]
fn blocked_delivers_no_reminder() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Blocked,
        &[("first", "in_progress")],
    );
    assert!(reminders(&mut state, &entity_id, &agent_id).is_empty());
}

/// An agent holding nothing open hears nothing.
#[test]
fn complete_holding_nothing_open_delivers_no_reminder() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
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
        DoneReport {
            status: DoneStatus::Completed,
            summary: "Nothing assigned to me.".into(),
            message_id: None,
        },
    );
    assert!(reminders(&mut state, &entity_id, &agent_id).is_empty());
}

/// An issue parked in Done is one the agent is finished with, so it is not
/// named — a reminder that is noise is one an agent answers without reading.
#[test]
fn an_issue_in_the_done_column_is_not_reminded_about() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("finished", "done"), ("still going", "in_progress")],
    );

    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 1, "{told:?}");
    let body = &told[0];
    assert!(body.contains("#2 still going"), "{body}");
    assert!(
        !body.contains("#1 finished"),
        "Done is finished with: {body}"
    );
    assert!(
        body.contains("1 issue assigned to you is still open"),
        "{body}"
    );
}

/// Answering a reminder with another Complete while still holding the same
/// issues is reminded again. That is the point, not a bug to suppress.
#[test]
fn a_second_complete_reminds_again() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("first", "in_progress")],
    );
    assert_eq!(reminders(&mut state, &entity_id, &agent_id).len(), 1);

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "Still done.".into(),
            message_id: None,
        },
    );
    assert_eq!(
        reminders(&mut state, &entity_id, &agent_id).len(),
        2,
        "it holds the same issue, so it is told again"
    );
}
