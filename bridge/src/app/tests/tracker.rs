//! The tracker's verbs, through the wire the browser calls (spec: Tasks).
//!
//! The scope rules and the refusals are the point: a task belongs to one
//! project, a link names something of that project, and a typed reference is
//! fenced by what the task is about.

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

/// The same, over a repository whose `main` tracks an origin — what a
/// workspace needs before Done will take it, since eligibility is the measure
/// that every commit is already somewhere else.
pub(super) fn tracked_with_origin(state_root: &Path) -> (AppState, String) {
    let repo = init_repo_named(state_root, "tracked");
    let origin = state_root.join("tracked.git");
    git_in(
        state_root,
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_in(&repo, &["fetch", "origin"]);
    git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
    let repo = std::fs::canonicalize(&repo).unwrap();
    let mut state = rooted(state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let project_id = added_project(&mut state, &repo);
    (state, project_id)
}

pub(super) fn filed(state: &mut AppState, project_id: &str, title: &str) -> Value {
    let created = state.handle(req(
        "tasks.create",
        json!({ "project_id": project_id, "title": title }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    created["result"]["task"].clone()
}

fn task_id(task: &Value) -> String {
    task["id"].as_str().unwrap().to_string()
}

/// The refusal one call answered with, as the client reads it.
pub(super) fn refused(state: &mut AppState, method: &str, params: Value) -> String {
    let answered = state.handle(req(method, params));
    assert_eq!(answered["ok"], false, "{answered:?}");
    answered["error"].as_str().unwrap_or_default().to_string()
}

/// A filed task carries a per-project number, starts in Backlog, is open,
/// belongs to the user, and says so on the wire as a `project_id` — never as
/// the path the record is keyed by.
#[test]
fn a_filed_task_is_numbered_open_and_in_the_first_column() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let task = filed(&mut state, &project_id, "Kanban drag does not persist");
    assert_eq!(task["number"], 1);
    assert_eq!(task["state"], "open");
    assert_eq!(task["status"], "backlog");
    assert_eq!(task["priority"], "none");
    assert_eq!(task["assignee"], Value::Null);
    assert_eq!(task["closed_at"], Value::Null);
    assert_eq!(task["created_by"], json!({ "kind": "user" }));
    assert_eq!(task["project_id"], project_id.as_str());
    assert!(
        task.get("project_path").is_none(),
        "the record's key never reaches the wire: {task:?}"
    );
    assert!(task["id"].as_str().unwrap().starts_with("task-"));

    let second = filed(&mut state, &project_id, "second");
    assert_eq!(second["number"], 2);
}

/// The timeline is the spread form — the record itself with one more key
/// naming which it is — ascending, with the `created` event first.
#[test]
fn a_new_tasks_timeline_opens_with_the_event_that_created_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let task = filed(&mut state, &project_id, "one");

    let read = state.handle(req("tasks.get", json!({ "task_id": task_id(&task) })));
    assert_eq!(read["ok"], true, "{read:?}");
    let timeline = read["result"]["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 1, "{timeline:?}");
    assert_eq!(timeline[0]["type"], "event");
    assert_eq!(timeline[0]["kind"], "created");
    assert_eq!(timeline[0]["actor"], json!({ "kind": "user" }));
    assert!(
        timeline[0]["id"].as_str().unwrap().starts_with("te-"),
        "the entry carries the record's own fields: {:?}",
        timeline[0]
    );
    assert_eq!(read["result"]["task"]["id"], task["id"]);
}

/// An update applies only what it names, and writes an event only for the
/// changes a timeline has to carry.
#[test]
fn an_update_writes_events_for_the_moves_and_not_for_the_wording() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let task = filed(&mut state, &project_id, "one");
    let id = task_id(&task);

    let worded = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "title": "one, renamed", "body": "why", "priority": "high" }),
    ));
    assert_eq!(worded["ok"], true, "{worded:?}");
    assert_eq!(worded["result"]["task"]["title"], "one, renamed");
    assert_eq!(worded["result"]["task"]["priority"], "high");
    assert_eq!(
        state.handle(req("tasks.get", json!({ "task_id": id })))["result"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "wording writes no event"
    );

    let moved = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "In review", "labels": ["bug"] }),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    assert_eq!(
        moved["result"]["task"]["status"], "in_review",
        "a display name normalizes to its slug"
    );
    let timeline = state.handle(req("tasks.get", json!({ "task_id": id })));
    let kinds: Vec<&str> = timeline["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["created", "labelled", "moved"]);

    // Moving where it already is changes nothing and says nothing.
    state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "in_review" }),
    ));
    assert_eq!(
        state.handle(req("tasks.get", json!({ "task_id": id })))["result"]["timeline"]
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
    let id = task_id(&filed(&mut state, &project_id, "one"));

    let done = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "done" }),
    ));
    assert_eq!(done["result"]["task"]["state"], "open", "still open");

    let closed = state.handle(req(
        "tasks.close",
        json!({ "task_id": id, "reason": "shipped" }),
    ));
    assert_eq!(closed["ok"], true, "{closed:?}");
    assert_eq!(closed["result"]["task"]["state"], "closed");
    assert_eq!(
        closed["result"]["task"]["status"], "done",
        "left where it was"
    );
    assert!(closed["result"]["task"]["closed_at"].is_string());

    assert!(refused(&mut state, "tasks.close", json!({ "task_id": id })).contains("already closed"));

    let reopened = state.handle(req("tasks.reopen", json!({ "task_id": id })));
    assert_eq!(reopened["result"]["task"]["state"], "open");
    assert_eq!(reopened["result"]["task"]["closed_at"], Value::Null);
    assert!(refused(&mut state, "tasks.reopen", json!({ "task_id": id })).contains("already open"));
}

/// Closing a task that is already closed is a conflict rather than a
/// no-op, and the code says so.
#[test]
fn a_refused_state_change_is_a_conflict_and_not_an_internal_error() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = task_id(&filed(&mut state, &project_id, "one"));
    state.handle(req("tasks.close", json!({ "task_id": id })));

    let answered = state.handle(req("tasks.close", json!({ "task_id": id })));
    assert_eq!(answered["error_code"], "conflict", "{answered:?}");

    let unknown = state.handle(req("tasks.get", json!({ "task_id": "task-nobody" })));
    assert_eq!(unknown["error_code"], "not_found", "{unknown:?}");

    let bad_column = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "icebox" }),
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
    let first = task_id(&filed(&mut state, &project_id, "first"));
    let second = task_id(&filed(&mut state, &project_id, "second"));
    state.handle(req(
        "tasks.update",
        json!({ "task_id": first, "labels": ["bug"], "status": "ready" }),
    ));
    state.handle(req("tasks.close", json!({ "task_id": second })));

    let numbers = |answered: &Value| -> Vec<u64> {
        answered["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["number"].as_u64().unwrap())
            .collect()
    };

    let all = state.handle(req("tasks.list", json!({ "project_id": project_id })));
    assert_eq!(numbers(&all), vec![2, 1], "newest first");

    let open = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "state": "open" }),
    ));
    assert_eq!(numbers(&open), vec![1]);

    let ready = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "status": "Ready" }),
    ));
    assert_eq!(numbers(&ready), vec![1], "a display name filters too");

    let labelled = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "label": "BUG" }),
    ));
    assert_eq!(numbers(&labelled), vec![1], "labels match as they dedupe");

    let unassigned = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "assignee": "none" }),
    ));
    assert_eq!(numbers(&unassigned), vec![2, 1]);
    let held = state.handle(req(
        "tasks.list",
        json!({ "project_id": project_id, "assignee": "any" }),
    ));
    assert!(numbers(&held).is_empty(), "nobody holds one yet");
}

/// A link names something of the task's own project, and nothing else.
#[test]
fn a_link_must_name_something_of_this_tasks_project() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = task_id(&filed(&mut state, &project_id, "one"));
    let ws = workspace(&mut state, &project_id, "here");

    let linked = link_task(
        &mut state,
        json!({ "task_id": id, "workspace_id": ws, "branch": "build/x" }),
    );
    assert_eq!(linked["ok"], true, "{linked:?}");
    assert_eq!(
        linked["result"]["task"]["links"]["workspace_ids"],
        json!([ws])
    );
    assert_eq!(
        linked["result"]["task"]["links"]["branches"],
        json!(["build/x"])
    );

    // Linking the same workspace again adds nothing and says nothing.
    link_task(&mut state, json!({ "task_id": id, "workspace_id": ws }));
    let kinds: Vec<String> = state.handle(req("tasks.get", json!({ "task_id": id })))["result"]
        ["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(kinds, vec!["created", "linked", "linked"], "{kinds:?}");

    assert!(link_refusal(
        &mut state,
        json!({ "task_id": id, "workspace_id": "ws-nobody" })
    )
    .contains("unknown workspace_id"));
    assert!(
        link_refusal(&mut state, json!({ "task_id": id, "commit": "nothex" }))
            .contains("commit link is invalid")
    );
    assert!(link_refusal(&mut state, json!({ "task_id": id })).contains("name a workspace_id"));
}

/// A parent is one task of the same project, never itself, and never a link
/// that closes a loop.
#[test]
fn a_parent_link_refuses_itself_and_refuses_a_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let parent = task_id(&filed(&mut state, &project_id, "parent"));
    let child = task_id(&filed(&mut state, &project_id, "child"));

    assert!(link_refusal(
        &mut state,
        json!({ "task_id": child, "parent_task_id": child })
    )
    .contains("its own parent"));

    let linked = link_task(
        &mut state,
        json!({ "task_id": child, "parent_task_id": parent }),
    );
    assert_eq!(linked["ok"], true, "{linked:?}");
    assert_eq!(
        linked["result"]["task"]["links"]["parent_task_id"],
        parent.as_str()
    );

    let cycle = link_refusal(
        &mut state,
        json!({ "task_id": parent, "parent_task_id": child }),
    );
    assert!(cycle.contains("close a loop"), "{cycle}");
}

/// A comment's typed references are fenced twice: by shape, then by what the
/// task is about.
#[test]
fn a_comments_references_are_fenced_by_shape_and_then_by_the_task() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let id = task_id(&filed(&mut state, &project_id, "one"));

    // Shape: a path that climbs out of a checkout is refused wherever it
    // arrives, in the same words a thread message gets.
    let escaping = refused(
        &mut state,
        "tasks.comment",
        json!({ "task_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "../../etc/passwd" }
        ]}),
    );
    assert!(escaping.contains("escapes the worktree"), "{escaping}");

    // Ownership: a file path means nothing until the task says which checkout.
    let unrooted = refused(
        &mut state,
        "tasks.comment",
        json!({ "task_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "bridge/src/app.rs" }
        ]}),
    );
    assert!(unrooted.contains("link the workspace"), "{unrooted}");

    let ws = workspace(&mut state, &project_id, "here");
    link_task(&mut state, json!({ "task_id": id, "workspace_id": ws }));
    let accepted = state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "bridge/src/app.rs", "line_start": 1, "line_end": 4 }
        ]}),
    ));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(
        accepted["result"]["comment"]["author"],
        json!({ "kind": "user" })
    );
    assert_eq!(accepted["result"]["comment"]["task_id"], id.as_str());

    // A commit has to be one the task links.
    let sha = "c8381faa9b1d4e6f2a0c7b5e3d8f1a2c4b6d8e0f";
    let stray = refused(
        &mut state,
        "tasks.comment",
        json!({ "task_id": id, "body": "at", "refs": [{ "kind": "commit", "sha": sha }] }),
    );
    assert!(stray.contains("not one this task links"), "{stray}");
    link_task(&mut state, json!({ "task_id": id, "commit": sha }));
    let now_known = state.handle(req(
        "tasks.comment",
        json!({ "task_id": id, "body": "at", "refs": [{ "kind": "commit", "sha": sha }] }),
    ));
    assert_eq!(now_known["ok"], true, "{now_known:?}");

    // The plan flow's kinds are refused outright: the tracker does not extend it.
    let plan_link = refused(
        &mut state,
        "tasks.comment",
        json!({ "task_id": id, "body": "stage", "refs": [
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
    let id = task_id(&filed(&mut state, &project_id, "one"));

    refused(
        &mut state,
        "tasks.comment",
        json!({ "task_id": id, "body": "look", "refs": [
            { "kind": "file", "path": "../escape" }
        ]}),
    );
    assert_eq!(
        state.handle(req("tasks.get", json!({ "task_id": id })))["result"]["timeline"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "only the created event"
    );
}

/// Two projects on one device keep two trackers, and neither verb reaches the
/// other's tasks.
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

    let listed = state.handle(req("tasks.list", json!({ "project_id": first })));
    let ids: Vec<&str> = listed["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![here["id"].as_str().unwrap()]);

    // A task names its own project whichever project asked for the list.
    assert_eq!(
        state.handle(req("tasks.get", json!({ "task_id": task_id(&there) })))["result"]["task"]
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

    let answered = state.handle(req("tasks.columns", json!({ "project_id": project_id })));
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
        "tasks.columns",
        json!({ "project_id": "proj-nobody" })
    )
    .contains("unknown project_id"));
}

/// The tracker and the retired plan flow do not touch: filing a task leaves
/// the plan tables alone, and the retirement guard does not catch `tasks.*`.
#[test]
fn the_tracker_does_not_reach_the_retired_plan_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    filed(&mut state, &project_id, "one");

    assert!(state.plans.is_empty(), "a tracker task is not a plan");
}

// ------------------------------------------------- attachments (#57) ---
//
// Files filed WITH a task. The bytes go up first and the task names them,
// exactly as a message's do — so a filed task can never point at an upload
// that failed halfway, and a file that will not land is refused on its own
// rather than failing the filing.

/// One file up, as the composer sends it. Answers the descriptor.
pub(super) fn attached(
    state: &mut AppState,
    project_id: &str,
    filename: &str,
    bytes: &[u8],
) -> Value {
    let answered = state.handle(req(
        "tasks.attach",
        json!({
            "project_id": project_id,
            "filename": filename,
            "content_b64": crate::encoding::b64encode(bytes),
        }),
    ));
    assert_eq!(answered["ok"], true, "{answered:?}");
    answered["result"].clone()
}

/// The upload names the file, types it, sizes it, and puts it somewhere that
/// exists — content-addressed, so the same bytes twice cost one copy.
#[test]
fn a_file_filed_with_a_task_lands_named_typed_and_sized() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let stored = attached(&mut state, &project_id, "board.png", b"\x89PNG\r\n\x1a\nxx");
    assert_eq!(stored["name"], "board.png");
    assert_eq!(stored["mime"], "image/png");
    assert_eq!(stored["size"], 10);
    let path = stored["path"].as_str().unwrap();
    assert!(
        std::path::Path::new(path).is_file(),
        "the bytes are on disk before anything names them: {path}"
    );

    // The same bytes again are the same leaf: one screenshot on three tasks
    // costs one copy.
    let again = attached(&mut state, &project_id, "board.png", b"\x89PNG\r\n\x1a\nxx");
    assert_eq!(again["path"], stored["path"]);
}

/// A name from another machine is a NAME here, never a location.
#[test]
fn a_filename_that_means_a_path_is_flattened_to_a_leaf() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let stored = attached(&mut state, &project_id, "../../etc/passwd", b"root:x:0:0");
    assert_eq!(stored["name"], "passwd");
    let path = stored["path"].as_str().unwrap();
    assert!(
        path.contains("attachments"),
        "it landed in the store and nowhere else: {path}"
    );
}

/// The task carries what was filed with it, described from the bytes on disk
/// rather than from what the client said about them.
#[test]
fn a_task_carries_the_files_it_was_filed_with() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let stored = attached(&mut state, &project_id, "board.png", b"\x89PNG\r\n\x1a\nxx");
    let created = state.handle(req(
        "tasks.create",
        json!({
            "project_id": project_id,
            "title": "Kanban drag does not persist",
            "attachments": [{ "path": stored["path"], "name": "board.png" }],
        }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let files = created["result"]["task"]["attachments"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{created:?}");
    assert_eq!(files[0]["name"], "board.png");
    assert_eq!(files[0]["mime"], "image/png");
    assert_eq!(files[0]["size"], 10);

    // And it is still there on the next read — the record holds it, not the call.
    let read = state.handle(req(
        "tasks.get",
        json!({ "task_id": created["result"]["task"]["id"] }),
    ));
    assert_eq!(read["result"]["task"]["attachments"], json!(files.clone()));
}

/// A task filed with nothing says so with an empty list rather than with a
/// missing key: a client that sent files and got no key back is looking at a
/// bridge that dropped them.
#[test]
fn a_task_with_no_files_answers_an_empty_list() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let task = filed(&mut state, &project_id, "one");
    assert_eq!(task["attachments"], json!([]));
}

/// A comment carries them too, which is how a file reaches a task that was
/// filed before anybody had it.
#[test]
fn a_comment_carries_the_files_said_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let task = filed(&mut state, &project_id, "one");

    let stored = attached(&mut state, &project_id, "trace.log", b"thread panicked");
    let said = state.handle(req(
        "tasks.comment",
        json!({
            "task_id": task["id"],
            "body": "Here is the trace.",
            "attachments": [{ "path": stored["path"] }],
        }),
    ));
    assert_eq!(said["ok"], true, "{said:?}");
    let files = said["result"]["comment"]["attachments"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{said:?}");
    assert_eq!(files[0]["size"], 15);
    // A caller that said no name gets the STORED leaf, hash and all — the same
    // fallback `thread.post` makes, kept the same on purpose. Every composer
    // sends the name `tasks.attach` answered, so this is the shape of a caller
    // that passed a bare path.
    let named = files[0]["name"].as_str().unwrap();
    assert!(named.ends_with("-trace.log"), "{named}");

    // And on the timeline, where a reader meets it.
    let read = state.handle(req("tasks.get", json!({ "task_id": task["id"] })));
    let commented = read["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "comment")
        .expect("the comment is on the timeline");
    assert_eq!(commented["attachments"], json!(files.clone()));
}

/// The bytes come back to a surface that cannot reach the disk.
#[test]
fn a_tasks_attachment_reads_back_through_the_task() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let stored = attached(&mut state, &project_id, "board.png", b"\x89PNG\r\n\x1a\nxx");
    let created = state.handle(req(
        "tasks.create",
        json!({
            "project_id": project_id,
            "title": "one",
            "attachments": [{ "path": stored["path"] }],
        }),
    ));
    let task_id = created["result"]["task"]["id"].clone();

    let read = state.handle(req(
        "tasks.attachment",
        json!({ "task_id": task_id, "path": stored["path"] }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(read["result"]["mime"], "image/png");
    assert_eq!(read["result"]["size"], 10);
    assert_eq!(
        crate::encoding::b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap(),
        b"\x89PNG\r\n\x1a\nxx".to_vec(),
        "the bytes that went up are the bytes that come back"
    );
}

/// Over the cap is refused with the number, before the write — a composer can
/// say why on the chip rather than after a slow encode and a round trip.
#[test]
fn a_file_over_the_cap_is_refused_with_its_size() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let huge = vec![0u8; (crate::app::ATTACHMENT_MAX_BYTES + 1) as usize];
    let refusal = refused(
        &mut state,
        "tasks.attach",
        json!({
            "project_id": project_id,
            "filename": "huge.bin",
            "content_b64": crate::encoding::b64encode(&huge),
        }),
    );
    assert!(refusal.contains("the limit is"), "{refusal}");

    let empty = refused(
        &mut state,
        "tasks.attach",
        json!({ "project_id": project_id, "filename": "nothing.txt", "content_b64": "" }),
    );
    assert!(empty.contains("attachment is empty"), "{empty}");
}

/// A path outside the store is not an attachment, whatever it is called.
///
/// The fence is containment after canonicalisation: a prefix check on the
/// string lets `..` walk straight out, and an "attachment" that reads any file
/// on the disk is an arbitrary-file read with a nice name.
#[test]
fn a_path_outside_the_store_is_not_an_attachment() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let task = filed(&mut state, &project_id, "one");

    let outsider = state_root.join("secret.txt");
    std::fs::write(&outsider, b"not yours").unwrap();

    // Filing with it refuses the whole call rather than filing a task whose
    // reason for being filed is missing from it.
    let refusal = refused(
        &mut state,
        "tasks.create",
        json!({
            "project_id": project_id,
            "title": "sneaky",
            "attachments": [{ "path": outsider.display().to_string() }],
        }),
    );
    assert!(refusal.contains("not an attachment"), "{refusal}");

    // And reading one back refuses too, including the `..` spelling of it.
    for path in [
        outsider.display().to_string(),
        "../../../etc/passwd".to_string(),
    ] {
        let refusal = refused(
            &mut state,
            "tasks.attachment",
            json!({ "task_id": task["id"], "path": path }),
        );
        assert!(refusal.contains("not an attachment"), "{path}: {refusal}");
    }
}

/// An agent attaches what the user sent IT: its copy is named relative to a
/// checkout, and the durable copy under the store answers for it.
#[test]
fn a_worktree_relative_path_resolves_to_the_durable_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    let stored = attached(&mut state, &project_id, "board.png", b"\x89PNG\r\n\x1a\nxx");
    let leaf = std::path::Path::new(stored["path"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let created = state.handle(req(
        "tasks.create",
        json!({
            "project_id": project_id,
            "title": "one",
            "attachments": [{ "path": format!(".build/attachments/{leaf}") }],
        }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let files = created["result"]["task"]["attachments"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{created:?}");
    assert_eq!(files[0]["size"], 10);
}
