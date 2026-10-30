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
