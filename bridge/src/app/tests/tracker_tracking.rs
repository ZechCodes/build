//! Who is watching an issue, and what one agent is on (spec: Issues →
//! Tracking).

use super::project_agent::{added_project, project_agent, workspace};
use super::tracker::{filed, tracked};
use super::*;

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
