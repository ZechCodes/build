use super::*;

#[test]
fn watched_task_capture_shares_unchanged_rows_and_keeps_old_values() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("plain");
    std::fs::create_dir(&repo).unwrap();
    let mut app = fixture(root.path());
    let project = app.add_project(repo, "main".into());
    for (id, title) in [("first", "First"), ("second", "Second")] {
        app.install_snapshot_watched_task(
            &project,
            id,
            Some(json!({"task_id": id, "title": title})),
        );
    }
    let old = app.snapshot_watched_task_shared_rows();
    let unchanged = app.snapshot_watched_task_shared_rows();
    assert_eq!(old.len(), 2);
    assert!(
        Arc::ptr_eq(&old[0], &unchanged[0]),
        "an unchanged task row must retain its immutable payload"
    );
    assert!(Arc::ptr_eq(&old[1], &unchanged[1]));

    app.install_snapshot_watched_task(
        &project,
        "second",
        Some(json!({"task_id": "second", "title": "Updated"})),
    );
    let updated = app.snapshot_watched_task_shared_rows();
    assert!(Arc::ptr_eq(&old[0], &updated[0]));
    assert!(!Arc::ptr_eq(&old[1], &updated[1]));
    assert_eq!(old[1]["title"], "Second");
    assert_eq!(updated[1]["title"], "Updated");
    assert_eq!(
        app.snapshot_watched_task_rows(),
        updated.iter().map(|row| row.as_ref().clone()).collect::<Vec<_>>()
    );

    app.install_snapshot_watched_task(&project, "first", None);
    let removed = app.snapshot_watched_task_shared_rows();
    assert_eq!(removed.len(), 1);
    assert!(Arc::ptr_eq(&updated[1], &removed[0]));
    assert_eq!(old[0]["title"], "First");
}
