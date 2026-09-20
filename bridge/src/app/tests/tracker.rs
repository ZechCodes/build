//! The tracker's verbs, through the wire the browser calls (spec: Issues).
//!
//! The scope rules and the refusals are the point: an issue belongs to one
//! project, a link names something of that project, and a typed reference is
//! fenced by what the issue is about.

use super::project_agent::{added_project, rooted, workspace};
use super::*;

/// A project with a store behind it — a tracker needs one, and a bridge
/// without persistence says so rather than losing what the user filed.
///
/// The repository's own temp dir comes back with the state: dropping it would
/// take the checkout the project is registered at out from under the test.
pub(super) fn tracked(state_root: &Path) -> (tempfile::TempDir, AppState, String) {
    let (home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut state = rooted(state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let project_id = added_project(&mut state, &repo);
    (home, state, project_id)
}

pub(super) fn filed(state: &mut AppState, project_id: &str, title: &str) -> Value {
    let created = state.handle(req(
        "issues.create",
        json!({ "project_id": project_id, "title": title }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"]["issue"].clone()
}

fn issue_id(issue: &Value) -> String {
    issue["id"].as_str().unwrap().to_string()
}

/// The refusal one call answered with, as the client reads it.
fn refused(state: &mut AppState, method: &str, params: Value) -> String {
    let answered = state.handle(req(method, params));
    assert_eq!(answered["ok"], false, "{answered:?}");
    answered["error"].as_str().unwrap_or_default().to_string()
}

/// A filed issue carries a per-project number, starts in Backlog, is open,
/// belongs to the user, and says so on the wire as a `project_id` — never as
/// the path the record is keyed by.
#[test]
fn a_filed_issue_is_numbered_open_and_in_the_first_column() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let issue = filed(&mut state, &project_id, "Kanban drag does not persist");
    assert_eq!(issue["number"], 1);
    assert_eq!(issue["state"], "open");
    assert_eq!(issue["status"], "backlog");
    assert_eq!(issue["priority"], "none");
    assert_eq!(issue["assignee"], Value::Null);
    assert_eq!(issue["closed_at"], Value::Null);
    assert_eq!(issue["created_by"], json!({ "kind": "user" }));
    assert_eq!(issue["project_id"], project_id.as_str());
    assert!(
        issue.get("project_path").is_none(),
        "the record's key never reaches the wire: {issue:?}"
    );
    assert!(issue["id"].as_str().unwrap().starts_with("issue-"));

    let second = filed(&mut state, &project_id, "second");
    assert_eq!(second["number"], 2);
}

/// The timeline is the spread form — the record itself with one more key
/// naming which it is — ascending, with the `created` event first.
#[test]
fn a_new_issues_timeline_opens_with_the_event_that_created_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let issue = filed(&mut state, &project_id, "one");

    let read = state.handle(req("issues.get", json!({ "issue_id": issue_id(&issue) })));
    assert_eq!(read["ok"], true, "{read:?}");
    let timeline = read["result"]["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 1, "{timeline:?}");
    assert_eq!(timeline[0]["type"], "event");
    assert_eq!(timeline[0]["kind"], "created");
    assert_eq!(timeline[0]["actor"], json!({ "kind": "user" }));
    assert!(
        timeline[0]["id"].as_str().unwrap().starts_with("ie-"),
        "the entry carries the record's own fields: {:?}",
        timeline[0]
    );
    assert_eq!(read["result"]["issue"]["id"], issue["id"]);
}

/// An update applies only what it names, and writes an event only for the
/// changes a timeline has to carry.
#[test]
fn an_update_writes_events_for_the_moves_and_not_for_the_wording() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let issue = filed(&mut state, &project_id, "one");
    let id = issue_id(&issue);

    let worded = state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "title": "one, renamed", "body": "why", "priority": "high" }),
    ));
    assert_eq!(worded["ok"], true, "{worded:?}");
    assert_eq!(worded["result"]["issue"]["title"], "one, renamed");
    assert_eq!(worded["result"]["issue"]["priority"], "high");
    assert_eq!(
        state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "wording writes no event"
    );

    let moved = state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "In review", "labels": ["bug"] }),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    assert_eq!(
        moved["result"]["issue"]["status"], "in_review",
        "a display name normalizes to its slug"
    );
    let timeline = state.handle(req("issues.get", json!({ "issue_id": id })));
    let kinds: Vec<&str> = timeline["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["created", "labelled", "moved"]);

    // Moving where it already is changes nothing and says nothing.
    state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "in_review" }),
    ));
    assert_eq!(
        state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "a timeline records changes, not requests"
    );
}

/// Closing and the Done column are independent: one says where the card is,
/// the other whether anyone is still expected to act.
#[test]
fn closing_does_not_move_the_card_and_moving_to_done_does_not_close_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    let done = state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "done" }),
    ));
    assert_eq!(done["result"]["issue"]["state"], "open", "still open");

    let closed = state.handle(req(
        "issues.close",
        json!({ "issue_id": id, "reason": "shipped" }),
    ));
    assert_eq!(closed["ok"], true, "{closed:?}");
    assert_eq!(closed["result"]["issue"]["state"], "closed");
    assert_eq!(
        closed["result"]["issue"]["status"], "done",
        "left where it was"
    );
    assert!(closed["result"]["issue"]["closed_at"].is_string());

    assert!(
        refused(&mut state, "issues.close", json!({ "issue_id": id })).contains("already closed")
    );

    let reopened = state.handle(req("issues.reopen", json!({ "issue_id": id })));
    assert_eq!(reopened["result"]["issue"]["state"], "open");
    assert_eq!(reopened["result"]["issue"]["closed_at"], Value::Null);
    assert!(
        refused(&mut state, "issues.reopen", json!({ "issue_id": id })).contains("already open")
    );
}

/// Closing an issue that is already closed is a conflict rather than a
/// no-op, and the code says so.
#[test]
fn a_refused_state_change_is_a_conflict_and_not_an_internal_error() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    state.handle(req("issues.close", json!({ "issue_id": id })));

    let answered = state.handle(req("issues.close", json!({ "issue_id": id })));
    assert_eq!(answered["error_code"], "conflict", "{answered:?}");

    let unknown = state.handle(req("issues.get", json!({ "issue_id": "issue-nobody" })));
    assert_eq!(unknown["error_code"], "not_found", "{unknown:?}");

    let bad_column = state.handle(req(
        "issues.update",
        json!({ "issue_id": id, "status": "icebox" }),
    ));
    assert_eq!(bad_column["error_code"], "invalid_params", "{bad_column:?}");
    assert!(
        bad_column["error"]
            .as_str()
            .unwrap()
            .contains("in_progress"),
        "the refusal names the columns there are: {bad_column:?}"
    );
}

/// The list is one project's, newest first, and every filter narrows it.
#[test]
fn the_list_is_one_projects_newest_first_and_every_filter_narrows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let first = issue_id(&filed(&mut state, &project_id, "first"));
    let second = issue_id(&filed(&mut state, &project_id, "second"));
    state.handle(req(
        "issues.update",
        json!({ "issue_id": first, "labels": ["bug"], "status": "ready" }),
    ));
    state.handle(req("issues.close", json!({ "issue_id": second })));

    let numbers = |answered: &Value| -> Vec<u64> {
        answered["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["number"].as_u64().unwrap())
            .collect()
    };

    let all = state.handle(req("issues.list", json!({ "project_id": project_id })));
    assert_eq!(numbers(&all), vec![2, 1], "newest first");

    let open = state.handle(req(
        "issues.list",
        json!({ "project_id": project_id, "state": "open" }),
    ));
    assert_eq!(numbers(&open), vec![1]);

    let ready = state.handle(req(
        "issues.list",
        json!({ "project_id": project_id, "status": "Ready" }),
    ));
    assert_eq!(numbers(&ready), vec![1], "a display name filters too");

    let labelled = state.handle(req(
        "issues.list",
        json!({ "project_id": project_id, "label": "BUG" }),
    ));
    assert_eq!(numbers(&labelled), vec![1], "labels match as they dedupe");

    let unassigned = state.handle(req(
        "issues.list",
        json!({ "project_id": project_id, "assignee": "none" }),
    ));
    assert_eq!(numbers(&unassigned), vec![2, 1]);
    let held = state.handle(req(
        "issues.list",
        json!({ "project_id": project_id, "assignee": "any" }),
    ));
    assert!(numbers(&held).is_empty(), "nobody holds one yet");
}

/// A link names something of the issue's own project, and nothing else.
#[test]
fn a_link_must_name_something_of_this_issues_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");

    let linked = state.handle(req(
        "issues.link",
        json!({ "issue_id": id, "workspace_id": ws, "branch": "build/x" }),
    ));
    assert_eq!(linked["ok"], true, "{linked:?}");
    assert_eq!(
        linked["result"]["issue"]["links"]["workspace_ids"],
        json!([ws])
    );
    assert_eq!(
        linked["result"]["issue"]["links"]["branches"],
        json!(["build/x"])
    );

    // Linking the same workspace again adds nothing and says nothing.
    state.handle(req(
        "issues.link",
        json!({ "issue_id": id, "workspace_id": ws }),
    ));
    let kinds: Vec<String> = state.handle(req("issues.get", json!({ "issue_id": id })))["result"]
        ["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(kinds, vec!["created", "linked", "linked"], "{kinds:?}");

    assert!(refused(
        &mut state,
        "issues.link",
        json!({ "issue_id": id, "workspace_id": "ws-nobody" })
    )
    .contains("unknown workspace_id"));
    assert!(refused(
        &mut state,
        "issues.link",
        json!({ "issue_id": id, "commit": "nothex" })
    )
    .contains("commit link is invalid"));
    assert!(
        refused(&mut state, "issues.link", json!({ "issue_id": id }))
            .contains("name a workspace_id")
    );
}

/// A parent is one issue of the same project, never itself, and never a link
/// that closes a loop.
#[test]
fn a_parent_link_refuses_itself_and_refuses_a_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let parent = issue_id(&filed(&mut state, &project_id, "parent"));
    let child = issue_id(&filed(&mut state, &project_id, "child"));

    assert!(refused(
        &mut state,
        "issues.link",
        json!({ "issue_id": child, "parent_issue_id": child })
    )
    .contains("its own parent"));

    let linked = state.handle(req(
        "issues.link",
        json!({ "issue_id": child, "parent_issue_id": parent }),
    ));
    assert_eq!(linked["ok"], true, "{linked:?}");
    assert_eq!(
        linked["result"]["issue"]["links"]["parent_issue_id"],
        parent.as_str()
    );

    let cycle = refused(
        &mut state,
        "issues.link",
        json!({ "issue_id": parent, "parent_issue_id": child }),
    );
    assert!(cycle.contains("close a loop"), "{cycle}");
}

/// A comment's typed references are fenced twice: by shape, then by what the
/// issue is about.
#[test]
fn a_comments_references_are_fenced_by_shape_and_then_by_the_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    // Shape: a path that climbs out of a checkout is refused wherever it
    // arrives, in the same words a thread message gets.
    let escaping = refused(
        &mut state,
        "issues.comment",
        json!({ "issue_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "../../etc/passwd" }
        ]}),
    );
    assert!(escaping.contains("escapes the worktree"), "{escaping}");

    // Ownership: a file path means nothing until the issue says which checkout.
    let unrooted = refused(
        &mut state,
        "issues.comment",
        json!({ "issue_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "bridge/src/app.rs" }
        ]}),
    );
    assert!(unrooted.contains("link the workspace"), "{unrooted}");

    let ws = workspace(&mut state, &project_id, "here");
    state.handle(req(
        "issues.link",
        json!({ "issue_id": id, "workspace_id": ws }),
    ));
    let accepted = state.handle(req(
        "issues.comment",
        json!({ "issue_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "bridge/src/app.rs", "line_start": 1, "line_end": 4 }
        ]}),
    ));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(
        accepted["result"]["comment"]["author"],
        json!({ "kind": "user" })
    );
    assert_eq!(accepted["result"]["comment"]["issue_id"], id.as_str());

    // A commit has to be one the issue links.
    let sha = "c8381faa9b1d4e6f2a0c7b5e3d8f1a2c4b6d8e0f";
    let stray = refused(
        &mut state,
        "issues.comment",
        json!({ "issue_id": id, "body": "at", "refs": [{ "kind": "commit", "sha": sha }] }),
    );
    assert!(stray.contains("not one this issue links"), "{stray}");
    state.handle(req("issues.link", json!({ "issue_id": id, "commit": sha })));
    let now_known = state.handle(req(
        "issues.comment",
        json!({ "issue_id": id, "body": "at", "refs": [{ "kind": "commit", "sha": sha }] }),
    ));
    assert_eq!(now_known["ok"], true, "{now_known:?}");

    // The plan flow's kinds are refused outright: the tracker does not extend it.
    let plan_link = refused(
        &mut state,
        "issues.comment",
        json!({ "issue_id": id, "body": "stage", "refs": [
            { "kind": "run", "run_id": "run-1" }
        ]}),
    );
    assert!(plan_link.contains("plan-flow references"), "{plan_link}");
}

/// A refused comment leaves nothing behind: half a comment is a comment whose
/// references lie about what it read.
#[test]
fn a_refused_comment_writes_neither_the_comment_nor_an_event() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = issue_id(&filed(&mut state, &project_id, "one"));

    refused(
        &mut state,
        "issues.comment",
        json!({ "issue_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "../escape" }
        ]}),
    );
    assert_eq!(
        state.handle(req("issues.get", json!({ "issue_id": id })))["result"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "only the created event"
    );
}

/// Two projects on one device keep two trackers, and neither verb reaches the
/// other's issues.
#[test]
fn two_projects_keep_two_trackers() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, first) = tracked(&state_root);
    let elsewhere = tempfile::tempdir().unwrap();
    let other_repo = init_repo_named(elsewhere.path(), "other");
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let second = added_project(&mut state, &other_repo);

    let here = filed(&mut state, &first, "here");
    let there = filed(&mut state, &second, "there");
    assert_eq!(here["number"], 1);
    assert_eq!(there["number"], 1, "each project counts from its own start");

    let listed = state.handle(req("issues.list", json!({ "project_id": first })));
    let ids: Vec<&str> = listed["result"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![here["id"].as_str().unwrap()]);

    // An issue names its own project whichever project asked for the list.
    assert_eq!(
        state.handle(req("issues.get", json!({ "issue_id": issue_id(&there) })))["result"]["issue"]
            ["project_id"],
        second.as_str()
    );
}

/// The columns are the board's, in board order, and a project that is not
/// registered is refused rather than answered for.
#[test]
fn the_columns_are_the_boards_and_an_unknown_project_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let answered = state.handle(req("issues.columns", json!({ "project_id": project_id })));
    let slugs: Vec<&str> = answered["result"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        slugs,
        vec!["backlog", "ready", "in_progress", "in_review", "done"]
    );
    assert_eq!(answered["result"]["columns"][2]["name"], "In progress");

    assert!(refused(
        &mut state,
        "issues.columns",
        json!({ "project_id": "proj-nobody" })
    )
    .contains("unknown project_id"));
}

/// The tracker and the retired plan flow do not touch: filing an issue leaves
/// the plan tables alone, and the retirement guard does not catch `issues.*`.
#[test]
fn the_tracker_does_not_reach_the_retired_plan_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    filed(&mut state, &project_id, "one");

    let plans = state.handle(req("issue.list", json!({})));
    assert_eq!(plans["ok"], true, "{plans:?}");
    assert!(
        plans["result"]["issues"]
            .as_array()
            .map(|issues| issues.is_empty())
            .unwrap_or(true),
        "a tracker issue is not a plan: {plans:?}"
    );

    // And the plan flow's own mutating verbs are still retired.
    let retired = state.handle(req("issue.create", json!({ "goal": "x" })));
    assert_eq!(retired["ok"], false, "{retired:?}");
}
