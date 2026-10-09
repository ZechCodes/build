//! Tracking ends with the work (#444): a task that reaches Done or is closed
//! tells its trackers once more and then has none.

use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::BridgeAction;
use crate::reviews::model::ReviewSnapshot;
use crate::tracker::Actor;

fn task_id(task: &Value) -> String {
    task["id"].as_str().unwrap().to_string()
}

fn read(state: &mut AppState, task_id: &str) -> Value {
    state.handle(req("tasks.get", json!({ "task_id": task_id })))["result"].clone()
}

fn trackers(state: &mut AppState, task_id: &str) -> Value {
    read(state, task_id)["task"]["trackers"].clone()
}

/// The `untracked` events on the timeline, as `(agent_id, by)`.
fn untracked(state: &mut AppState, task_id: &str) -> Vec<(String, String)> {
    read(state, task_id)["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "untracked")
        .map(|entry| {
            (
                entry["payload"]["agent_id"].as_str().unwrap().to_string(),
                entry["payload"]["by"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The task notices on one agent's conversation, oldest first.
fn notices(state: &mut AppState, who: &(String, String)) -> Vec<Value> {
    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": who.0, "agent_id": who.1, "limit": 50 }),
    ));
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"].clone())
        .filter(|message| message["from_build"] == true && message["task_notice"].is_object())
        .collect()
}

/// A task `watcher` tracks, and the watcher, with nothing else on either.
fn watched_task(state: &mut AppState, project_id: &str) -> ((String, String), String) {
    let watcher = coding_agent(state, project_id, "watcher");
    let id = task_id(&filed(state, project_id, "one"));
    let tracking = set_task_tracking(state, &id, &watcher.1, true);
    assert_eq!(tracking["ok"], true, "{tracking:?}");
    (watcher, id)
}

fn comment_as_user(state: &mut AppState, task_id: &str, body: &str) {
    let commented = state.handle(req(
        "tasks.comment",
        json!({ "task_id": task_id, "body": body }),
    ));
    assert_eq!(commented["ok"], true, "{commented:?}");
}

#[test]
fn a_move_to_done_tells_every_tracker_and_then_drops_them_all() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);
    let actor = coding_agent(&mut state, &project_id, "actor");
    set_task_tracking(&mut state, &id, &actor.1, true);

    let moved = call(
        &mut state,
        &actor,
        BridgeAction::TrackerMoveTask {
            task_id: id.clone(),
            status: "done".into(),
            track: None,
        },
    )
    .expect("the actor moves it to Done");

    assert_eq!(moved["task"]["trackers"], json!([]), "the answer says so");
    assert_eq!(trackers(&mut state, &id), json!([]));
    let told = notices(&mut state, &watcher);
    assert_eq!(told.len(), 1, "the Done move still arrives: {told:?}");
    assert_eq!(told[0]["task_notice"]["action"], "moved");
    assert_eq!(told[0]["task_notice"]["to"], "done");
    assert!(
        notices(&mut state, &actor).is_empty(),
        "the actor is dropped too, and still not told what it did"
    );
    assert_eq!(
        untracked(&mut state, &id),
        vec![
            (watcher.1.clone(), "finished".to_string()),
            (actor.1.clone(), "finished".to_string()),
        ],
        "the timeline says why each one stopped"
    );
}

#[test]
fn closing_a_task_tells_its_trackers_and_then_drops_them() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);

    let closed = state.handle(req("tasks.close", json!({ "task_id": id })));
    assert_eq!(closed["ok"], true, "{closed:?}");

    assert_eq!(closed["result"]["task"]["trackers"], json!([]));
    assert_eq!(trackers(&mut state, &id), json!([]));
    let told = notices(&mut state, &watcher);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["task_notice"]["action"], "closed");
    assert_eq!(
        untracked(&mut state, &id),
        vec![(watcher.1.clone(), "finished".to_string())]
    );
}

#[test]
fn a_later_change_to_a_finished_task_reaches_nobody_who_tracked_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);
    let moved = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "done" }),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    assert_eq!(notices(&mut state, &watcher).len(), 1, "the Done move");

    comment_as_user(&mut state, &id, "one more thing");
    let relabelled = state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "labels": ["later"] }),
    ));
    assert_eq!(relabelled["ok"], true, "{relabelled:?}");

    assert_eq!(
        notices(&mut state, &watcher).len(),
        1,
        "nothing after the Done move"
    );
}

#[test]
fn reopening_a_finished_task_does_not_bring_its_old_trackers_back() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);

    // Closed, then reopened.
    let (watcher, closed) = watched_task(&mut state, &project_id);
    state.handle(req("tasks.close", json!({ "task_id": closed })));
    let reopened = state.handle(req("tasks.reopen", json!({ "task_id": closed })));
    assert_eq!(reopened["ok"], true, "{reopened:?}");
    assert_eq!(reopened["result"]["task"]["trackers"], json!([]));
    assert_eq!(notices(&mut state, &watcher).len(), 1, "only the close");

    // Moved to Done, then moved back to a live column by an agent that asks
    // to follow it: that agent tracks it the usual way, and nobody else.
    let done = task_id(&filed(&mut state, &project_id, "two"));
    set_task_tracking(&mut state, &done, &watcher.1, true);
    state.handle(req(
        "tasks.update",
        json!({ "task_id": done, "status": "done" }),
    ));
    let mover = coding_agent(&mut state, &project_id, "mover");
    let back = call(
        &mut state,
        &mover,
        BridgeAction::TrackerMoveTask {
            task_id: done.clone(),
            status: "in_progress".into(),
            track: Some(true),
        },
    )
    .expect("moved back");
    assert_eq!(back["task"]["trackers"], json!([mover.1]));
    assert_eq!(trackers(&mut state, &done), json!([mover.1]));
}

#[test]
fn a_write_that_finishes_a_task_does_not_start_tracking_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let agent = coding_agent(&mut state, &project_id, "agent");

    let moved_id = task_id(&filed(&mut state, &project_id, "move it"));
    let moved = call(
        &mut state,
        &agent,
        BridgeAction::TrackerMoveTask {
            task_id: moved_id.clone(),
            status: "done".into(),
            track: Some(true),
        },
    )
    .expect("a move to Done that asks to follow");
    assert_eq!(moved["task"]["trackers"], json!([]));
    assert_eq!(trackers(&mut state, &moved_id), json!([]));

    let closed_id = task_id(&filed(&mut state, &project_id, "close it"));
    let closed = call(
        &mut state,
        &agent,
        BridgeAction::TrackerCloseTask {
            task_id: closed_id.clone(),
            reason: None,
            track: Some(true),
        },
    )
    .expect("a close that asks to follow");
    assert_eq!(closed["task"]["trackers"], json!([]));

    // create_task tracks by default, and a task filed straight into Done is
    // filed finished.
    let created = call(
        &mut state,
        &agent,
        BridgeAction::TrackerCreateTask {
            title: "already done".into(),
            body: None,
            status: Some("done".into()),
            labels: Vec::new(),
            priority: None,
            attachments: Vec::new(),
            track: None,
            notify_user: None,
            mention_user: None,
        },
    )
    .expect("filed in Done");
    assert_eq!(created["task"]["trackers"], json!([]));
    let created_id = task_id(&created["task"]);
    assert_eq!(trackers(&mut state, &created_id), json!([]));
}

#[test]
fn tracking_a_finished_task_on_purpose_still_follows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let agent = coding_agent(&mut state, &project_id, "agent");
    let id = task_id(&filed(&mut state, &project_id, "one"));
    state.handle(req(
        "tasks.update",
        json!({ "task_id": id, "status": "done" }),
    ));

    let tracking = call(
        &mut state,
        &agent,
        BridgeAction::TrackerTrackTask {
            task_id: id.clone(),
        },
    )
    .expect("track_task on a Done task");
    assert_eq!(tracking["task"]["trackers"], json!([agent.1]));

    comment_as_user(&mut state, &id, "still here");
    assert_eq!(
        trackers(&mut state, &id),
        json!([agent.1]),
        "a later write keeps it"
    );
    assert_eq!(
        notices(&mut state, &agent).len(),
        1,
        "and it hears the comment"
    );
}

#[test]
fn completing_a_review_tells_its_trackers_and_then_drops_them() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let workspace_id = workspace(&mut state, &project_id, "review");
    let (watcher, id) = watched_task(&mut state, &project_id);
    state
        .tracker_store()
        .unwrap()
        .save_review_snapshot(
            &id,
            &workspace_id,
            0,
            ReviewSnapshot {
                publication: None,
                id: format!("snapshot-{id}"),
                number: 0,
                created_at: crate::store::now_rfc3339(),
                author: Actor::User,
                directories: Vec::new(),
            },
        )
        .unwrap();

    state
        .review_complete(
            crate::api::v1::reviews::ReviewCompleteParams {
                task_id: id.clone(),
                expected_version: 1,
                description: "Looks right".into(),
            },
            Actor::User,
        )
        .expect("the review completes");

    assert_eq!(read(&mut state, &id)["task"]["status"], "done");
    assert_eq!(trackers(&mut state, &id), json!([]));
    let told = notices(&mut state, &watcher);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["task_notice"]["to"], "done");
    assert_eq!(
        untracked(&mut state, &id),
        vec![(watcher.1.clone(), "finished".to_string())]
    );
}

/// Tasks finished before the rule existed still carry their trackers. The
/// store drops them once, and a task tracked on purpose afterwards keeps its
/// tracker.
#[test]
fn trackers_left_on_finished_tasks_are_dropped_once() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, done) = watched_task(&mut state, &project_id);
    let live = task_id(&filed(&mut state, &project_id, "still going"));
    set_task_tracking(&mut state, &live, &watcher.1, true);
    let store = state.tracker_store().unwrap().clone();
    let mut stale = store.load_tracker_task(&done).unwrap().unwrap();
    stale.status = crate::tracker::DONE_STATUS.into();
    stale.done_at = Some(stale.updated_at.clone());
    store.save_tracker_task_activity(&stale, &[], &[]).unwrap();
    assert_eq!(
        trackers(&mut state, &done),
        json!([watcher.1]),
        "the old shape"
    );

    // The boot that opened this store already ran the pass, over nothing.
    assert_eq!(
        store.end_tracking_on_finished_tasks().unwrap(),
        0,
        "already run"
    );
    store.forget_ended_tracking();
    assert_eq!(store.end_tracking_on_finished_tasks().unwrap(), 1);

    assert_eq!(trackers(&mut state, &done), json!([]));
    assert_eq!(
        untracked(&mut state, &done),
        vec![(watcher.1.clone(), "finished".to_string())]
    );
    assert_eq!(
        trackers(&mut state, &live).as_array().unwrap().len(),
        1,
        "a live task keeps its tracker"
    );

    set_task_tracking(&mut state, &done, &watcher.1, true);
    assert_eq!(store.end_tracking_on_finished_tasks().unwrap(), 0, "once");
    assert_eq!(trackers(&mut state, &done), json!([watcher.1]));
}

/// A write built from a task read before it finished (an off-lock PR merge
/// can finish it in between) must not bring the trackers that finishing took
/// off back with it (#456).
#[test]
fn a_stale_write_after_finishing_does_not_restore_its_trackers() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);
    let store = state.tracker_store().unwrap().clone();
    let mut stale = store.load_tracker_task(&id).unwrap().unwrap();
    let moved = state.handle(req(
        "tasks.update",
        json!({"task_id": id, "status": "done"}),
    ));
    assert_eq!(moved["ok"], true);
    assert_eq!(trackers(&mut state, &id), json!([]));

    stale.labels.push("later".to_string());
    let now = crate::store::now_rfc3339();
    let labelled = crate::tracker::TaskEvent::new(
        &id,
        Actor::User,
        crate::tracker::TaskEventKind::Labelled,
        json!({"added": ["later"], "removed": []}),
        &now,
    );
    let (saved, _) = store
        .save_review_task_activity(&stale, &[], &[labelled], &Actor::User, None, &now)
        .unwrap();
    assert_eq!(saved.status, "done");
    assert!(saved.trackers.is_empty(), "restored: {:?}", saved.trackers);
    assert_eq!(trackers(&mut state, &id), json!([]));
    assert_eq!(
        saved.labels,
        vec!["later".to_string()],
        "the edit still lands"
    );

    // A stale write that tracks on purpose still adds that one.
    let mut tracking = stale.clone();
    tracking.track("agent-later").unwrap();
    let tracked = crate::tracker::TaskEvent::new(
        &id,
        Actor::User,
        crate::tracker::TaskEventKind::Tracked,
        json!({"agent_id": "agent-later"}),
        &now,
    );
    let (saved, _) = store
        .save_review_task_activity(&tracking, &[], &[tracked], &Actor::User, None, &now)
        .unwrap();
    assert_eq!(saved.trackers, vec!["agent-later".to_string()]);
    assert!(!saved.is_tracked_by(&watcher.1));
}

/// One update that relabels a task and moves it to Done tells its trackers
/// it moved to Done, since that is the last thing they will hear (#456).
#[test]
fn a_combined_edit_and_done_tells_trackers_it_moved_to_done() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);
    let moved = state.handle(req(
        "tasks.update",
        json!({"task_id": id, "labels": ["finished"], "status": "done"}),
    ));
    assert_eq!(moved["ok"], true, "{moved:?}");
    assert_eq!(trackers(&mut state, &id), json!([]));
    let told = notices(&mut state, &watcher);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["task_notice"]["action"], "moved", "{told:?}");
    assert_eq!(told[0]["task_notice"]["to"], "done");
}

/// The boot pass is bookkeeping: a watched finished task the user cleared
/// stays cleared, and its inbox row keeps its place (#456).
#[test]
fn the_boot_pass_leaves_cleared_inbox_rows_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_watcher, id) = watched_task(&mut state, &project_id);
    let latest = read(&mut state, &id)["timeline"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let store = state.tracker_store().unwrap().clone();
    let mut stale = store.load_tracker_task(&id).unwrap().unwrap();
    stale.status = crate::tracker::DONE_STATUS.into();
    stale.done_at = Some(stale.updated_at.clone());
    stale.dismissed_through = Some(latest);
    store.save_tracker_task_activity(&stale, &[], &[]).unwrap();
    let row = |state: &mut AppState| {
        state
            .watched_task_rows()
            .into_iter()
            .find(|row| row["task_id"] == id)
            .unwrap()
    };
    let before = row(&mut state);
    assert_eq!(before["done_until_next"], true);

    store.forget_ended_tracking();
    assert_eq!(store.end_tracking_on_finished_tasks().unwrap(), 1);
    let after = row(&mut state);
    assert_eq!(after["done_until_next"], true, "reopened: {after}");
    assert_eq!(after["anchor"], before["anchor"]);
    assert_eq!(after["last_event"], before["last_event"]);
}

/// A watched task moved to Done reads as that move in the inbox, not as the
/// tracking Build ended behind it.
#[test]
fn a_watched_task_moved_to_done_reads_as_the_move() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (_watcher, id) = watched_task(&mut state, &project_id);
    assert_eq!(
        state.handle(req("tasks.watch", json!({"task_id": id})))["ok"],
        true
    );
    let moved = state.handle(req(
        "tasks.update",
        json!({"task_id": id, "status": "done"}),
    ));
    assert_eq!(moved["ok"], true);
    let row = state
        .watched_task_rows()
        .into_iter()
        .find(|row| row["task_id"] == id)
        .unwrap();
    assert_ne!(row["last_event"]["text"], "Updated", "{row}");
    assert_eq!(row["last_event"]["actor"], "you", "{row}");
}

/// Done twice, and closed then Done, end nothing an agent tracked on purpose
/// after the task first finished.
#[test]
fn finishing_a_finished_task_again_keeps_tracking_asked_for_since() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let (watcher, id) = watched_task(&mut state, &project_id);
    assert_eq!(
        state.handle(req("tasks.close", json!({"task_id": id})))["ok"],
        true
    );
    set_task_tracking(&mut state, &id, &watcher.1, true);
    for _ in 0..2 {
        assert_eq!(
            state.handle(req(
                "tasks.update",
                json!({"task_id": id, "status": "done"})
            ))["ok"],
            true
        );
        assert_eq!(trackers(&mut state, &id), json!([watcher.1]));
    }
    assert_eq!(untracked(&mut state, &id).len(), 1);
}
