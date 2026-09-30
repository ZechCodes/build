//! `workspace.measure_sizes` and the size walks it queues (#273): the
//! Workspaces tab asks, the walks run one at a time off the app mutex, and
//! each size lands on the workspace's lifecycle record.

use super::workspace_reclaim::{
    build_output_in, call, impatient, lifecycle, linked_workspace, now_ms, root_and_checkout,
};
use super::*;
use crate::reclaim::ReclaimPolicy;
use std::sync::{Arc, Mutex};

fn measure_sizes(state: &Arc<Mutex<AppState>>, params: Value) -> Value {
    let answer = call(state, "workspace.measure_sizes", params);
    assert_eq!(answer["ok"], true, "{answer:?}");
    answer["result"].clone()
}

fn queued(result: &Value) -> Vec<String> {
    result["queued"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

/// Every walk the queue holds, run here the way the service thread runs them.
fn walk_everything_queued(state: &Arc<Mutex<AppState>>) -> usize {
    let mut walked = 0;
    while AppState::measure_next_workspace_size(state, now_ms()) {
        walked += 1;
    }
    walked
}

/// The verb answers at once: nothing is walked in the request, and the
/// workspace is named as queued.
#[test]
fn a_request_queues_a_walk_and_answers_at_once() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();

    let result = measure_sizes(&state, json!({ "workspace_ids": [ws] }));

    assert_eq!(queued(&result), vec![ws.clone()]);
    assert_eq!(
        lifecycle(&state, &ws),
        Value::Null,
        "nothing is measured inside the request"
    );
}

/// The walk writes the size and when it was measured on the lifecycle record,
/// before any sweep has run, and says nothing else about the workspace.
#[test]
fn a_walk_lands_the_size_on_the_lifecycle_record() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    let before = now_ms();

    assert_eq!(walk_everything_queued(&state), 1);

    let verdict = lifecycle(&state, &ws);
    assert!(verdict["size_bytes"].as_u64().unwrap() > 0, "{verdict:?}");
    assert!(
        verdict["size_measured_at_ms"].as_i64().unwrap() >= before,
        "{verdict:?}"
    );
    assert_eq!(verdict["idle"], false, "a size is not a verdict");
    assert_eq!(verdict["reclaimable"], false);
}

/// Each measured size is announced: the workspace list moves, so every
/// subscribed client is sent the rows again.
#[test]
fn a_measured_size_is_announced_on_the_board() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    let before = state.lock().unwrap().changes.board_revision();

    walk_everything_queued(&state);

    assert!(state.lock().unwrap().changes.board_revision() > before);
}

/// No ids is every workspace Build made; a project id narrows it to that
/// project's.
#[test]
fn no_ids_queues_every_workspace_and_a_project_narrows_it() {
    let (_tmp, state, project, ws, _task) = linked_workspace();

    assert_eq!(
        queued(&measure_sizes(
            &state,
            json!({ "project_id": "proj-elsewhere" })
        )),
        Vec::<String>::new()
    );
    assert_eq!(
        queued(&measure_sizes(&state, json!({ "project_id": project }))),
        vec![ws.clone()]
    );
    walk_everything_queued(&state);
    assert!(lifecycle(&state, &ws)["size_bytes"].as_u64().unwrap() > 0);
}

/// An id that names no workspace is passed over, not refused: the tab asks
/// for what it last saw, which may since have been removed.
#[test]
fn an_unknown_workspace_is_passed_over() {
    let (_tmp, state, _project, _ws, _task) = linked_workspace();

    let result = measure_sizes(&state, json!({ "workspace_ids": ["ws-gone"] }));

    assert_eq!(queued(&result), Vec::<String>::new());
    assert_eq!(walk_everything_queued(&state), 0);
}

/// Asking again while a walk is queued for the workspace queues nothing more.
#[test]
fn a_request_while_a_walk_is_queued_is_a_no_op() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));

    let again = measure_sizes(&state, json!({ "workspace_ids": [ws] }));

    assert_eq!(queued(&again), Vec::<String>::new());
    assert_eq!(walk_everything_queued(&state), 1, "one walk, not two");
}

/// A size measured within the last few minutes is reused, not walked again.
#[test]
fn a_recent_size_is_reused() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    let measured = lifecycle(&state, &ws);

    let again = measure_sizes(&state, json!({ "workspace_ids": [ws] }));

    assert_eq!(queued(&again), Vec::<String>::new());
    assert_eq!(walk_everything_queued(&state), 0);
    assert_eq!(lifecycle(&state, &ws), measured);
}

/// A size older than the reuse window is walked again.
#[test]
fn an_old_size_is_walked_again() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    let window = i64::try_from(crate::reclaim::SIZE_REUSE_WINDOW.as_millis()).unwrap();
    state
        .lock()
        .unwrap()
        .workspace_lifecycle
        .get_mut(&ws)
        .unwrap()
        .size_measured_at_ms = Some(now_ms() - window - 1);

    assert_eq!(
        queued(&measure_sizes(&state, json!({ "workspace_ids": [ws] }))),
        vec![ws]
    );
}

/// A walk that runs out of budget keeps the last size and the time it was
/// measured: nothing it half-counted is written.
#[test]
fn a_walk_out_of_budget_keeps_the_last_size() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    let measured = lifecycle(&state, &ws);
    // Stale enough to walk again, with a checkout that has grown since.
    state
        .lock()
        .unwrap()
        .workspace_lifecycle
        .get_mut(&ws)
        .unwrap()
        .size_measured_at_ms = Some(0);
    let stale = lifecycle(&state, &ws);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    build_output_in(&checkout);
    state.lock().unwrap().reclaim_policy = ReclaimPolicy {
        measure_entries: 1,
        ..ReclaimPolicy::default()
    };

    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    assert_eq!(walk_everything_queued(&state), 1);

    assert_eq!(lifecycle(&state, &ws), stale);
    assert_eq!(stale["size_bytes"], measured["size_bytes"]);
}

/// A workspace whose walk ran out is not stuck: the next request walks it.
#[test]
fn a_walk_out_of_budget_can_be_asked_for_again() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    state.lock().unwrap().reclaim_policy = ReclaimPolicy {
        measure_entries: 1,
        ..ReclaimPolicy::default()
    };
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    assert_eq!(lifecycle(&state, &ws), Value::Null, "nothing to keep yet");

    assert_eq!(
        queued(&measure_sizes(&state, json!({ "workspace_ids": [ws] }))),
        vec![ws]
    );
}

/// A sweep that does not size the workspace keeps the size the tab asked
/// for, and when it was measured.
#[test]
fn a_sweep_keeps_the_measured_size() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    let measured = lifecycle(&state, &ws);

    AppState::sweep_workspaces(&state, &ReclaimPolicy::default(), now_ms());

    let swept = lifecycle(&state, &ws);
    assert_eq!(swept["idle"], false, "{swept:?}");
    assert_eq!(swept["size_bytes"], measured["size_bytes"]);
    assert_eq!(
        swept["size_measured_at_ms"],
        measured["size_measured_at_ms"]
    );
}

/// A sweep that sizes a quiet workspace says when it did.
#[test]
fn a_sweep_that_sizes_a_workspace_says_when() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    let now = now_ms();

    AppState::sweep_workspaces(&state, &impatient(), now);

    assert_eq!(lifecycle(&state, &ws)["size_measured_at_ms"], now);
}

/// The size a walk measured survives a restart like the rest of the record.
#[test]
fn a_measured_size_is_kept_across_a_restart() {
    let (tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    walk_everything_queued(&state);
    let before = lifecycle(&state, &ws);
    drop(state);

    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let reopened = super::project_agent::rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let reopened = Arc::new(Mutex::new(reopened));
    assert_eq!(lifecycle(&reopened, &ws), before);
}

/// A workspace removed while its walk waited is dropped from the queue, and
/// no record is written for it.
#[test]
fn a_workspace_removed_while_queued_is_not_written() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    measure_sizes(&state, json!({ "workspace_ids": [ws] }));
    let deleted = call(&state, "workspace.delete", json!({ "workspace_id": ws }));
    assert_eq!(deleted["ok"], true, "{deleted:?}");

    walk_everything_queued(&state);

    assert!(!state.lock().unwrap().workspace_lifecycle.contains_key(&ws));
}
