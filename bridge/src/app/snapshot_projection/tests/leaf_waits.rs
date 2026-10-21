//! Real publication leaf contention must remain visible inside an app guard.
use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn pending_owner_leaf_wait_stays_with_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    app.snapshot_observations.ready = true;
    assert_blocked_acquisition(
        &app.snapshot_observations.pending_owners,
        "pending-owner",
        || app.mark_snapshot_owner_pending("owner"),
    );
    assert!(app
        .snapshot_observations
        .pending_owners
        .lock()
        .unwrap()
        .contains("owner"));
}

#[test]
fn ownership_getter_leaf_wait_stays_with_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    assert_blocked_acquisition(
        &app.snapshot_observations.ownership,
        "ownership-getter",
        || assert!(app.snapshot_ownership_context().is_none()),
    );
}

#[test]
fn ownership_cleanup_leaf_wait_stays_with_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    let ownership = app.begin_snapshot_ownership();
    assert_blocked_acquisition(
        &app.snapshot_observations.ownership,
        "ownership-cleanup",
        || drop(ownership),
    );
    assert!(app.snapshot_ownership_context().is_none());
}

#[test]
fn observed_project_leaf_wait_stays_with_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    let row = json!({"project_id": "project", "name": "accepted"});
    assert_blocked_acquisition(
        &app.snapshot_observations.projects,
        "observed-project",
        || app.record_project_snapshot_observation("project", &row),
    );
    assert_eq!(
        app.snapshot_observations.projects.lock().unwrap()["project"].value,
        row
    );
}

#[test]
fn watched_task_leaf_wait_stays_with_the_original_write() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    let row = json!({"task_id": "task", "title": "accepted"});
    assert_blocked_acquisition(
        &app.snapshot_observations.watched_tasks,
        "observed-task",
        || app.install_snapshot_watched_task("project", "task", Some(row.clone())),
    );
    assert_eq!(
        *app.snapshot_observations.watched_tasks.lock().unwrap()["project"][0],
        row
    );
}
