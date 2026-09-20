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
                track: None,
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
    assert_eq!(
        body, "The actor agent moved #1 Kanban drag to In review",
        "one line: who did what to which issue"
    );

    // And the same thing structured, so a client draws that line with a link
    // rather than parsing it back out of prose (spec: Issues → Tracking).
    assert_eq!(
        notice["issue_notice"],
        json!({
            "actor": { "kind": "agent", "agent_id": actor.1 },
            "action": "moved",
            "from": "backlog",
            "to": "in_review",
        }),
        "{notice:?}"
    );

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
        // The report moves the NEWEST issue the agent holds to In review, so
        // the last one listed is the one this Complete hands over.
        &[
            ("first", "in_progress"),
            ("second", "ready"),
            ("just finished", "in_progress"),
        ],
    );

    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 1, "one reminder per Complete: {told:?}");
    let body = &told[0];
    assert!(body.contains("#1 first (In progress)"), "{body}");
    assert!(body.contains("#2 second (Ready)"), "{body}");
    // The case that made every Complete nag about the work it had just handed
    // over: the report's own move put #3 in In review, and In review is the
    // agent saying it is finished.
    assert!(
        !body.contains("#3 just finished"),
        "In review is not held open: {body}"
    );
    assert!(
        body.contains("2 issues assigned to you are still open"),
        "{body}"
    );
}

/// In review means the agent has said the work is ready to be looked at, and
/// deciding it is done is somebody else's. So it is not held, and an agent
/// that reports Complete holding nothing else hears nothing at all.
#[test]
fn an_issue_in_review_is_not_held_open_by_the_agent() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("handed over", "in_review")],
    );
    assert!(
        reminders(&mut state, &entity_id, &agent_id).is_empty(),
        "nothing is waiting on the agent, so nothing is said"
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
        &[
            ("finished", "done"),
            ("still going", "in_progress"),
            ("just finished", "in_progress"),
        ],
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
        !body.contains("#3 just finished"),
        "and so is In review: {body}"
    );
    assert!(
        body.contains("1 issue assigned to you is still open"),
        "{body}"
    );
}

/// Answering a reminder with another Complete while holding the SAME issues
/// says nothing the second time.
///
/// The reminder is delivered as a turn, so an agent that answers it reports
/// Complete again — which is another reminder, which is another answer. That
/// loop ran five times on #27 before the agent stopped replying. The nudge is
/// worth sending when the set changes and worth nothing when it has not.
#[test]
fn a_second_complete_holding_the_same_issues_says_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[("second", "in_progress"), ("first", "in_progress")],
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
        1,
        "the same set is not worth saying twice"
    );
}

/// But a set that CHANGED is news again: an agent handed a second issue after
/// being reminded about the first is told about both.
#[test]
fn a_complete_holding_a_different_set_reminds_again() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (entity_id, agent_id) = reported(
        &mut state,
        &project_id,
        DoneStatus::Completed,
        &[
            ("a", "in_progress"),
            ("b", "in_progress"),
            ("just finished", "in_progress"),
        ],
    );
    assert_eq!(reminders(&mut state, &entity_id, &agent_id).len(), 1);

    // One of the two it was told about is finished with, so the set has moved.
    let listed = state.handle(req("issues.list", json!({ "project_id": project_id })));
    let done = listed["result"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|issue| issue["title"] == "b")
        .and_then(|issue| issue["id"].as_str())
        .unwrap()
        .to_string();
    state.handle(req(
        "issues.update",
        json!({ "issue_id": done, "status": "done" }),
    ));

    state.done_deferring_for_agent(
        &entity_id,
        &agent_id,
        DoneReport {
            status: DoneStatus::Completed,
            summary: "And that one is finished with.".into(),
            message_id: None,
        },
    );
    let told = reminders(&mut state, &entity_id, &agent_id);
    assert_eq!(told.len(), 2, "the set moved, so it is said again");
    assert!(told[1].contains("#1 a"), "{told:?}");
    assert!(!told[1].contains("#2 b"), "{told:?}");
}

// ------------------------------------------ the agent says what it did ---

/// The `issue_action` messages on one agent's own conversation.
fn said(state: &mut AppState, entity_id: &str, agent_id: &str) -> Vec<Value> {
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
        .filter(|message| message["issue_action"].is_object())
        .collect()
}

/// Every tool an agent writes with posts exactly one message saying what it
/// did, authored by the agent and not marked as Build's.
#[test]
fn each_tool_posts_one_message_saying_what_the_agent_did() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");

    let filed_by_agent = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            crate::mcp::BridgeAction::TrackerCreateIssue {
                title: "Kanban drag".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: None,
            },
        )
        .expect("an agent files an issue");
    let id = filed_by_agent["issue"]["id"].as_str().unwrap().to_string();

    for (action, expected) in [
        (
            crate::mcp::BridgeAction::TrackerMoveIssue {
                issue_id: id.clone(),
                status: "in_review".into(),
                track: None,
            },
            "moved",
        ),
        (
            crate::mcp::BridgeAction::TrackerCommentIssue {
                issue_id: id.clone(),
                body: "Reproduced it.".into(),
                refs: Vec::new(),
                track: None,
            },
            "commented_on",
        ),
        (
            crate::mcp::BridgeAction::TrackerCloseIssue {
                issue_id: id.clone(),
                reason: None,
                track: None,
            },
            "closed",
        ),
    ] {
        state
            .on_agent_mcp_action(&who.0, &who.1, action)
            .unwrap_or_else(|why| panic!("{expected}: {why}"));
    }

    let messages = said(&mut state, &who.0, &who.1);
    let actions: Vec<&str> = messages
        .iter()
        .map(|message| message["issue_action"]["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        vec!["created", "moved", "commented_on", "closed"],
        "one message per write, in the order they happened"
    );

    let first = &messages[0];
    assert_eq!(first["role"], "agent", "the agent's own words");
    assert!(
        first["from_build"] != true,
        "not Build's sentence: {first:?}"
    );
    assert_eq!(first["issue_action"]["issue_id"], id.as_str());
    assert_eq!(first["issue_action"]["number"], 1);
    assert_eq!(first["issue_action"]["title"], "Kanban drag");
    assert_eq!(first["body"], "Created #1 Kanban drag");
}

/// A comment's message carries the id that deep-links the comment itself.
#[test]
fn a_comment_message_carries_its_comment_id() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let commented = state
        .on_agent_mcp_action(
            &who.0,
            &who.1,
            crate::mcp::BridgeAction::TrackerCommentIssue {
                issue_id: id.clone(),
                body: "Reproduced it.".into(),
                refs: Vec::new(),
                track: None,
            },
        )
        .expect("an agent comments");
    let comment_id = commented["comment"]["id"].as_str().unwrap().to_string();

    let messages = said(&mut state, &who.0, &who.1);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0]["issue_action"]["comment_id"],
        comment_id.as_str(),
        "the id that links the comment rather than the issue"
    );
    assert_eq!(messages[0]["body"], "Commented on #1 one");
}

/// The api path posts nothing: a human moving a card is already looking at
/// the board.
#[test]
fn the_api_path_posts_no_action_message() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "in_review" }),
    ));
    state.handle(req(
        "issues.comment",
        json!({ "issue_id": id, "body": "from the board" }),
    ));

    assert!(
        said(&mut state, &who.0, &who.1).is_empty(),
        "the human is already looking at the board"
    );
}

/// The message is in the ACTING agent's conversation, not the assignee's.
#[test]
fn the_message_lands_on_the_actor_and_not_on_the_assignee() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let actor = coding_agent(&mut state, &project_id, "actor");
    let target = coding_agent(&mut state, &project_id, "target");
    let id = issue_id(&filed(&mut state, &project_id, "hand this over"));

    state
        .on_agent_mcp_action(
            &actor.0,
            &actor.1,
            crate::mcp::BridgeAction::TrackerAssignIssue {
                issue_id: id.clone(),
                assignee: json!({ "kind": "agent", "agent_id": target.1 }),
                note: None,
                track: None,
            },
        )
        .expect("an agent hands work over");

    let by_actor = said(&mut state, &actor.0, &actor.1);
    assert_eq!(by_actor.len(), 1, "{by_actor:?}");
    assert_eq!(by_actor[0]["issue_action"]["action"], "assigned");
    assert_eq!(by_actor[0]["body"], "Assigned #1 hand this over");

    assert!(
        said(&mut state, &target.0, &target.1).is_empty(),
        "the assignee gets the ISSUE, not a note about somebody assigning it"
    );
}

// ------------------------------------------- track in the same call ---

/// An issue an agent FILES tracks it, without being asked.
///
/// The default that matters (spec: Issues → Tracking). An agent that files an
/// issue almost always wants to know how it goes, and the one that filed and
/// assigned twelve in an afternoon heard nothing about any of them.
#[test]
fn an_issue_an_agent_files_tracks_it_unless_it_says_otherwise() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "filer");

    let filed = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCreateIssue {
                title: "Kanban drag".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: None,
            },
        )
        .expect("an agent may file an issue");
    let id = filed["issue"]["id"].as_str().unwrap().to_string();
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);
    assert_eq!(
        filed["issue"]["trackers"],
        json!([caller.1]),
        "and the answer says so, rather than making the caller read it back"
    );

    // Said otherwise, it does not.
    let quiet = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCreateIssue {
                title: "not my problem".into(),
                body: None,
                status: None,
                labels: Vec::new(),
                priority: None,
                track: Some(false),
            },
        )
        .expect("an agent may file one it does not want to hear about");
    let quiet_id = quiet["issue"]["id"].as_str().unwrap().to_string();
    assert_eq!(trackers(&mut state, &quiet_id), Vec::<String>::new());
}

/// Every other write takes `track: true` and defaults to false: an agent that
/// moves somebody else's card in passing has not asked to follow it.
#[test]
fn any_other_write_can_start_tracking_in_the_same_call() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));

    // Without the flag, nothing is followed.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerMoveIssue {
                issue_id: id.clone(),
                status: "ready".into(),
                track: None,
            },
        )
        .expect("a move");
    assert_eq!(trackers(&mut state, &id), Vec::<String>::new());

    // With it, once — and the answer carries the trackers as they now stand.
    let moved = state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerMoveIssue {
                issue_id: id.clone(),
                status: "in_progress".into(),
                track: Some(true),
            },
        )
        .expect("a move that follows");
    assert_eq!(moved["issue"]["trackers"], json!([caller.1]));
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);

    // Idempotent, the way track_issue is: asking twice is not two trackers,
    // and writes no second `tracked` event.
    let before = event_kinds(&mut state, &id).len();
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerCommentIssue {
                issue_id: id.clone(),
                body: "still on it".into(),
                refs: Vec::new(),
                track: Some(true),
            },
        )
        .expect("a comment that follows");
    assert_eq!(trackers(&mut state, &id), vec![caller.1.clone()]);
    assert_eq!(
        event_kinds(&mut state, &id).len(),
        before,
        "a set does not record being told twice"
    );
}

/// The api path is a human on the board, who is not an agent and cannot be
/// put on an issue's trackers. It carries no such field and is unaffected.
#[test]
fn the_api_path_tracks_nobody() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let made = state.handle(req(
        "issues.create",
        json!({ "project_id": project_id, "title": "filed by the user", "track": true }),
    ));
    assert_eq!(made["ok"], true, "{made:?}");
    assert_eq!(
        made["result"]["issue"]["trackers"],
        json!([]),
        "an unknown field is ignored, and the user is not a tracker"
    );
}

/// An agent's own line says who it handed the issue TO, not just that it
/// assigned it.
#[test]
fn an_assignment_the_agent_made_records_who_got_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ws = workspace(&mut state, &project_id, "here");
    let caller = coding_agent(&mut state, &project_id, "caller");
    let id = issue_id(&filed(&mut state, &project_id, "Kanban drag"));

    // Handed to a new agent on an existing workspace: a creating kind, which
    // resolves to the agent it makes.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerAssignIssue {
                issue_id: id.clone(),
                assignee: json!({ "kind": "new_agent", "workspace_id": ws }),
                note: None,
                track: None,
            },
        )
        .expect("an agent may assign");

    let lines = said(&mut state, &caller.0, &caller.1);
    let assigned = lines
        .iter()
        .find(|line| line["issue_action"]["action"] == "assigned")
        .unwrap_or_else(|| panic!("no assignment line: {lines:?}"));
    let got = assigned["issue_action"]["assignee"].clone();
    assert_eq!(got["kind"], "agent", "{assigned:?}");
    assert!(
        got["agent_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("agent-")),
        "the agent the dispatch made, not the kind that asked for it: {assigned:?}"
    );

    // Handing it back is its own word, and goes to nobody.
    state
        .on_agent_mcp_action(
            &caller.0,
            &caller.1,
            crate::mcp::BridgeAction::TrackerAssignIssue {
                issue_id: id.clone(),
                assignee: Value::Null,
                note: None,
                track: None,
            },
        )
        .expect("an agent may hand it back");
    let lines = said(&mut state, &caller.0, &caller.1);
    let handed_back = lines
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("no line: {lines:?}"));
    assert_eq!(handed_back["issue_action"]["action"], "unassigned");
    assert!(
        handed_back["issue_action"]["assignee"].is_null(),
        "nobody has it: {handed_back:?}"
    );
    assert_eq!(handed_back["body"], "Unassigned #1 Kanban drag");
}
