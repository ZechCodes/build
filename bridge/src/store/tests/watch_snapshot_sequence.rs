//! Canonical watch sequence must share the transaction that writes task data.
use super::*;
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind};

fn sequence(store: &Store) -> u64 {
    let value: String = store
        .connection()
        .query_row("SELECT value FROM meta WHERE key='watch_snapshot_seq'", [], |row| row.get(0))
        .expect("the durable watch sequence is initialized");
    let parsed = value.parse::<u64>().expect("sequence is a u64");
    assert_eq!(value, parsed.to_string(), "sequence is canonical decimal TEXT");
    parsed
}

#[test]
fn task_creation_and_activity_advance_once_per_canonical_transaction() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let before = sequence(&store);
    let draft = Task::drafted("/sequence", "before", Actor::User, support::NOW);
    let event = TaskEvent::new(&draft.id, Actor::User, TaskEventKind::Created, serde_json::json!({}), support::NOW);
    let mut task = store.create_tracker_task(draft, &[event]).unwrap();
    assert_eq!(sequence(&store), before.checked_add(1).unwrap());
    let after_create = sequence(&store);
    task.watched = true;
    task.title = "changed".into();
    let event = TaskEvent::new(&task.id, Actor::User, TaskEventKind::Watched, serde_json::json!({}), support::NOW);
    store.save_tracker_task_activity(&task, &[], &[event]).unwrap();
    assert_eq!(sequence(&store), after_create.checked_add(1).unwrap());
}

#[test]
fn sequence_overflow_rolls_back_task_and_history_but_not_unrelated_work() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let mut task = store.create_tracker_task(Task::drafted("/sequence", "before", Actor::User, support::NOW), &[]).unwrap();
    store.connection().execute("INSERT OR REPLACE INTO meta(key,value) VALUES('watch_snapshot_seq',?1)", [u64::MAX.to_string()]).unwrap();
    let before_timeline = store.load_tracker_timeline(&task.id).unwrap().len();
    task.title = "must roll back".into();
    task.watched = true;
    let event = TaskEvent::new(&task.id, Actor::User, TaskEventKind::Watched, serde_json::json!({}), support::NOW);
    assert!(store.save_tracker_task_activity(&task, &[], &[event]).is_err(), "overflow must refuse the covered transaction");
    let stored = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_eq!(stored.title, "before");
    assert!(!stored.watched);
    assert_eq!(store.load_tracker_timeline(&task.id).unwrap().len(), before_timeline);
    assert_eq!(sequence(&store), u64::MAX);
    store.in_transaction(|tx| {
        tx.execute("INSERT INTO meta(key,value) VALUES('unrelated_test_value','ok')", [])?;
        Ok(())
    }).expect("unrelated work remains available at sequence maximum");
    assert_eq!(sequence(&store), u64::MAX);
}

#[test]
fn rolled_back_savepoint_at_max_allows_unrelated_commit() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let task = store.create_tracker_task(Task::drafted("/sequence", "before", Actor::User, support::NOW), &[]).unwrap();
    store.connection().execute("INSERT OR REPLACE INTO meta(key,value) VALUES('watch_snapshot_seq',?1)", [u64::MAX.to_string()]).unwrap();
    let mut changed = task.clone();
    changed.title = "rolled back".into();
    store.in_transaction(|tx| {
        tx.execute_batch("SAVEPOINT covered_change")?;
        super::super::tracker::write_tracker_task(tx, &changed)?;
        tx.execute_batch("ROLLBACK TO covered_change; RELEASE covered_change")?;
        tx.execute("INSERT INTO meta(key,value) VALUES('unrelated_after_savepoint','ok')", [])?;
        Ok(())
    }).expect("rolled-back covered write must not block unrelated work at max");
    assert_eq!(store.load_tracker_task(&task.id).unwrap().unwrap().title, "before");
    let value: String = store.connection().query_row("SELECT value FROM meta WHERE key='unrelated_after_savepoint'", [], |row| row.get(0)).unwrap();
    assert_eq!(value, "ok");
    assert_eq!(sequence(&store), u64::MAX);
}

#[test]
fn write_after_rebuild_checkpoint_remains_in_final_projection() {
    use std::{sync::mpsc, time::Duration};

    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let mut task = store.create_tracker_task(Task::drafted("/sequence", "before", Actor::User, support::NOW), &[]).unwrap();
    let rowid: i64 = store.connection().query_row("SELECT rowid FROM tracker_tasks WHERE id=?1", [&task.id], |row| row.get(0)).unwrap();
    store.connection().execute("INSERT OR REPLACE INTO meta(key,value) VALUES('task_summary_rebuild','1')", []).unwrap();
    let (reached_tx, reached_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (held_tx, held_rx) = mpsc::channel();
    let database = root.path().join("build.db");
    let rebuild = std::thread::spawn(move || -> Result<(), StoreError> {
        super::super::task_summaries::checkpoint_test::arm(reached_tx, release_rx, held_tx);
        let mut conn = rusqlite::Connection::open(database)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        if super::super::task_summaries::initialize(&mut conn)? {
            super::super::task_summaries::mark_complete(&mut conn)?;
        }
        Ok(())
    });
    struct RebuildGuard {
        release: Option<mpsc::Sender<()>>,
        worker: Option<std::thread::JoinHandle<Result<(), StoreError>>>,
    }
    impl RebuildGuard {
        fn finish(&mut self) -> std::thread::Result<Result<(), StoreError>> {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
            self.worker.take().unwrap().join()
        }
    }
    impl Drop for RebuildGuard {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    let mut guard = RebuildGuard { release: Some(release_tx), worker: Some(rebuild) };
    let checkpoint = reached_rx.recv_timeout(Duration::from_secs(10));
    if !checkpoint.as_ref().is_ok_and(|through| *through >= rowid) {
        let _ = guard.finish();
        panic!("task checkpoint was not reached: {checkpoint:?}");
    }
    task.title = "written after checkpoint".into();
    task.watched = true;
    let event = TaskEvent::new(&task.id, Actor::User, TaskEventKind::Watched, serde_json::json!({}), support::NOW);
    let write = store.save_tracker_task_activity(&task, &[], &[event]);
    let rebuilt = guard.finish().expect("rebuild thread did not panic");
    let held_until_release = held_rx.recv_timeout(Duration::from_secs(1));
    assert_eq!(held_until_release, Ok(true), "rebuild barrier timed out or lost its witness before release");
    assert!(write.is_ok(), "normal write must remain available during rebuild: {write:?}");
    assert!(rebuilt.is_ok(), "rebuild failed: {rebuilt:?}");
    assert_eq!(store.load_tracker_task(&task.id).unwrap().unwrap().title, "written after checkpoint");
    let raw: String = store.connection().query_row("SELECT record FROM tracker_task_summaries WHERE task_id=?1", [&task.id], |row| row.get(0)).unwrap();
    let row: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(row["title"], "written after checkpoint", "projection lost the later canonical write");
    assert_eq!(row["watched"], true, "projection lost watch state");
}
