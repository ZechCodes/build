//! Exercise supported canonical writers, rather than manually staging a batch.
use super::*;
use crate::store::Store;
use crate::tracker::{Actor, Task, TaskComment};

const NOW: &str = "2026-01-01T00:00:00Z";
fn events(store: &Store) -> Vec<(i64, String, String)> {
    store
        .connection()
        .prepare("SELECT seq,entity_key,operation FROM sync_journal ORDER BY seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn create(store: &Store) -> Task {
    store
        .create_tracker_task(
            Task::drafted("/synthetic-project", "one", Actor::User, NOW),
            &[],
        )
        .unwrap()
}

#[test]
fn canonical_create_edit_and_unchanged_save_publish_once_per_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    assert_eq!(events(&store).len(), 1, "create must journal");
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(events(&store).len(), 1, "unchanged save must be quiet");
    task.title = "two".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(events(&store).len(), 2);
    task.body = "a body edit with the same timestamp".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        events(&store).len(),
        3,
        "task detail edits must invalidate the opened task"
    );
    let routes: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sync_routes WHERE stream_kind='task' AND stream_key=?1",
            [&task.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(routes, 3);
}

#[test]
fn multiple_activity_appends_in_one_transaction_have_one_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store);
    let comments:Vec<TaskComment>=(0..10).map(|n|serde_json::from_value(json!({"id":format!("synthetic-comment-{n}"),"task_id":task.id,"created_at":NOW,"author":{"kind":"user"},"body":"note"})).unwrap()).collect();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "one final task publication per SQL transaction"
    );
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "duplicate comment ids must be quiet"
    );
}

#[test]
fn canonical_task_and_journal_both_rollback_on_append_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    store.connection().execute_batch("CREATE TRIGGER synthetic_reject_sync BEFORE INSERT ON sync_journal BEGIN SELECT RAISE(ABORT,'synthetic journal failure'); END;").unwrap();
    task.title = "must rollback".into();
    assert!(store.save_tracker_task_activity(&task, &[], &[]).is_err());
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().title,
        "one"
    );
    assert_eq!(events(&store).len(), 1);
}
