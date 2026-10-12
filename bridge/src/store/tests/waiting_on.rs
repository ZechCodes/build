use super::*;
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind, TaskState, TimelineEntry};

const PROJECT: &str = "/waiting-project";
const NOW: &str = "2026-10-12T19:00:00Z";

fn filed(store: &Store, title: &str) -> Task {
    store
        .create_tracker_task(Task::drafted(PROJECT, title, Actor::User, NOW), &[])
        .unwrap()
}

fn add(store: &Store, task: &Task, ids: &[String]) {
    store
        .edit_waiting_on(PROJECT, &task.id, ids, &[], &Actor::User, NOW)
        .unwrap();
    drain(store);
}

fn drain(store: &Store) {
    while store.reconcile_waiting_on(2).unwrap().pending {}
}

fn change(store: &Store, task: &mut Task, status: &str, state: TaskState) -> TaskEvent {
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from":task.status,"to":status}),
        NOW,
    );
    task.status = status.into();
    task.state = state;
    store
        .save_tracker_task_activity(task, &[], std::slice::from_ref(&event))
        .unwrap();
    drain(store);
    event
}

fn events(store: &Store, task: &Task, kind: TaskEventKind) -> Vec<TaskEvent> {
    store
        .load_tracker_timeline(&task.id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            TimelineEntry::Event(event) if event.kind == kind => Some(event),
            _ => None,
        })
        .collect()
}

#[test]
fn waiting_on_same_project_dag_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = filed(&store, "a");
    let b = filed(&store, "b");
    let foreign = store
        .create_tracker_task(Task::drafted("/foreign", "secret", Actor::User, NOW), &[])
        .unwrap();
    let missing = "task-missing".to_string();
    for invalid in [&foreign.id, &missing, &a.id] {
        assert!(store
            .edit_waiting_on(
                PROJECT,
                &a.id,
                &[b.id.clone(), invalid.clone()],
                &[],
                &Actor::User,
                NOW
            )
            .is_err());
        assert!(store
            .waiting_on_summary(PROJECT, &a.id)
            .unwrap()
            .waiting_on
            .is_empty());
    }
    add(&store, &a, &[b.id.clone(), b.id.clone()]);
    drain(&store);
    assert_eq!(
        store
            .waiting_on_summary(PROJECT, &a.id)
            .unwrap()
            .waiting_on
            .len(),
        1
    );
    assert!(store
        .edit_waiting_on(PROJECT, &b.id, &[a.id.clone()], &[], &Actor::User, NOW)
        .is_err());
    let draft = Task::drafted(PROJECT, "refused", Actor::User, NOW);
    assert!(store
        .create_tracker_task_waiting_on(draft.clone(), &[], &[b.id.clone(), foreign.id.clone()])
        .is_err());
    assert!(store.load_tracker_task(&draft.id).unwrap().is_none());
    assert!(store
        .edit_waiting_on(PROJECT, &a.id, &[], &[foreign.id], &Actor::User, NOW)
        .is_err());
    let ids: Vec<_> = (0..21)
        .map(|n| filed(&store, &format!("blocker {n}")).id)
        .collect();
    assert!(store
        .edit_waiting_on(PROJECT, &b.id, &ids, &[], &Actor::User, NOW)
        .is_err());
    assert!(store
        .waiting_on_summary(PROJECT, &b.id)
        .unwrap()
        .waiting_on
        .is_empty());
}

#[test]
fn waiting_on_all_blockers_required() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = filed(&store, "a");
    let mut b = filed(&store, "b");
    let mut c = filed(&store, "c");
    add(&store, &a, &[b.id.clone(), c.id.clone()]);
    drain(&store);
    change(&store, &mut b, "done", TaskState::Open);
    assert!(store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    assert!(events(&store, &a, TaskEventKind::Unblocked).is_empty());
    let cause = change(&store, &mut c, "done", TaskState::Open);
    assert!(!store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    let unblocked = events(&store, &a, TaskEventKind::Unblocked);
    assert_eq!(unblocked.len(), 1);
    assert_eq!(
        unblocked[0].payload["resolved_task_ids"],
        serde_json::json!([b.id, c.id])
    );
    assert_eq!(
        unblocked[0].payload["cause_event_ids"],
        serde_json::json!([cause.id])
    );
    drain(&store);
    assert_eq!(events(&store, &a, TaskEventKind::Unblocked).len(), 1);
}

#[test]
fn waiting_on_close_resolution_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = filed(&store, "a");
    let mut b = filed(&store, "closed earlier");
    let mut c = filed(&store, "last");
    add(&store, &a, &[b.id.clone(), c.id.clone()]);
    drain(&store);
    change(&store, &mut b, "backlog", TaskState::Closed);
    change(&store, &mut c, "done", TaskState::Closed);
    let summary = store.waiting_on_summary(PROJECT, &a.id).unwrap();
    assert_eq!(summary.waiting_on[0].resolved_by.as_deref(), Some("close"));
    assert_eq!(summary.waiting_on[1].resolved_by.as_deref(), Some("done"));
    let unblocked = events(&store, &a, TaskEventKind::Unblocked);
    assert_eq!(
        unblocked[0].payload["closed_without_done_task_ids"],
        serde_json::json!([b.id])
    );
    change(&store, &mut b, "backlog", TaskState::Open);
    assert!(store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    store
        .edit_waiting_on(PROJECT, &a.id, &[], &[b.id.clone()], &Actor::User, NOW)
        .unwrap();
    drain(&store);
    let unblocked = events(&store, &a, TaskEventKind::Unblocked);
    assert_eq!(
        unblocked[1].payload["removed_task_ids"],
        serde_json::json!([b.id])
    );
    assert_eq!(
        unblocked[1].payload["resolved_task_ids"],
        serde_json::json!([c.id])
    );
}

#[test]
fn waiting_on_lifecycle_paths() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut a = filed(&store, "a");
    let mut b = filed(&store, "b");
    add(&store, &a, &[b.id.clone()]);
    drain(&store);
    let mut moved = a.clone();
    moved.status = "in_progress".into();
    assert!(store.save_tracker_task_activity(&moved, &[], &[]).is_err());
    change(&store, &mut b, "done", TaskState::Closed);
    change(&store, &mut b, "done", TaskState::Open);
    assert!(!store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    change(&store, &mut b, "backlog", TaskState::Closed);
    assert!(!store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    change(&store, &mut b, "backlog", TaskState::Open);
    assert!(store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    change(&store, &mut a, "done", TaskState::Open);
    assert!(!store.waiting_on_summary(PROJECT, &a.id).unwrap().blocked);
    assert!(
        store
            .edit_waiting_on(PROJECT, &b.id, &[a.id], &[], &Actor::User, NOW)
            .is_err(),
        "resolved edges still participate in cycles"
    );
}

#[test]
fn waiting_on_migration_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut old = filed(&store, "old assignment");
    old.assignee = Some(crate::tracker::Assignee::ProjectAgent);
    store.save_tracker_task_activity(&old, &[], &[]).unwrap();
    store.set_schema_version(15);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let summary = store.waiting_on_summary(PROJECT, &old.id).unwrap();
    assert!(!summary.blocked);
    assert!(summary.waiting_on.is_empty());
    assert!(store
        .reconcile_waiting_on(2)
        .unwrap()
        .transitions
        .is_empty());
    assert!(events(&store, &old, TaskEventKind::Unblocked).is_empty());
}

#[test]
fn waiting_on_concurrent_opposite_edges_cannot_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = filed(&store, "a");
    let b = filed(&store, "b");
    let other = Store::new(dir.path()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let thread = |store: Store, from: String, to: String| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            store.edit_waiting_on(PROJECT, &from, &[to], &[], &Actor::User, NOW).is_ok()
        })
    };
    let first = thread(store.clone(), a.id.clone(), b.id.clone());
    let second = thread(other, b.id.clone(), a.id.clone());
    assert_ne!(first.join().unwrap(), second.join().unwrap(), "exactly one opposite edge commits");
    let count = store.waiting_on_summary(PROJECT, &a.id).unwrap().waiting_on.len()
        + store.waiting_on_summary(PROJECT, &b.id).unwrap().waiting_on.len();
    assert_eq!(count, 1);
}

#[test]
fn waiting_on_fan_out_resumes_after_restart_and_invalidates_titles() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut blocker = filed(&store, "before");
    let tasks: Vec<_> = (0..11).map(|n| {
        let task = filed(&store, &format!("dependent {n}"));
        add(&store, &task, &[blocker.id.clone()]);
        task
    }).collect();
    store.take_waiting_on_changes(100).unwrap();
    blocker.title = "renamed".into();
    blocker.status = "done".into();
    store.save_tracker_task_activity(&blocker, &[], &[]).unwrap();
    assert!(store.reconcile_waiting_on(2).unwrap().pending);
    assert_eq!(events(&store, &tasks[0], TaskEventKind::Unblocked).len(), 1);
    assert_eq!(events(&store, &tasks[10], TaskEventKind::Unblocked).len(), 0);
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    drain(&store);
    let changes = store.take_waiting_on_changes(100).unwrap();
    for task in &tasks {
        assert_eq!(events(&store, task, TaskEventKind::Unblocked).len(), 1);
        assert_eq!(store.waiting_on_summary(PROJECT, &task.id).unwrap().waiting_on[0].title, "renamed");
        assert!(changes.iter().any(|(_, id)| id == &task.id));
    }
    let page = store.waiting_on_blocks(PROJECT, &blocker.id, 3, None).unwrap();
    assert_eq!(page.blocks.iter().map(|d| d.number).collect::<Vec<_>>(), vec![2,3,4]);
    let cursor = page.next_cursor.unwrap();
    let next = store.waiting_on_blocks(PROJECT, &blocker.id, 100, Some(&cursor)).unwrap();
    assert_eq!(next.blocks.len(), 8);
    assert!(store.waiting_on_blocks(PROJECT, &tasks[0].id, 3, Some(&cursor)).is_err());
    assert!(store.waiting_on_blocks("/foreign", &blocker.id, 3, None).is_err());
    let summaries = store.waiting_on_summaries(PROJECT, &tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>()).unwrap();
    assert!(summaries.values().all(|s| !s.blocked && s.waiting_on.len() == 1));
    assert_eq!(store.waiting_on_summary(PROJECT, &blocker.id).unwrap().blocks_count, 11);
}

#[test]
fn waiting_on_mixed_link_write_and_reconciliation_page_roll_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let a = filed(&store, "original");
    let mut b = filed(&store, "blocker");
    let foreign = store.create_tracker_task(Task::drafted("/foreign", "secret", Actor::User, NOW), &[]).unwrap();
    let mut changed = a.clone();
    changed.title = "must roll back".into();
    changed.links.branches.push("new-link".into());
    assert!(store.save_tracker_task_waiting_on(&changed, &[], &[], &[b.id.clone(), foreign.id], &[], &Actor::User).is_err());
    assert_eq!(store.load_tracker_task(&a.id).unwrap().unwrap(), a);
    add(&store, &a, &[b.id.clone()]);
    b.status = "done".into();
    store.save_tracker_task_activity(&b, &[], &[]).unwrap();
    store.connection().execute_batch("CREATE TRIGGER refuse_unblock BEFORE INSERT ON tracker_dependency_outbox BEGIN SELECT RAISE(ABORT, 'fault'); END;").unwrap();
    assert!(store.reconcile_waiting_on(2).is_err());
    assert!(events(&store, &a, TaskEventKind::Unblocked).is_empty());
    store.connection().execute_batch("DROP TRIGGER refuse_unblock;").unwrap();
    drain(&store);
    assert_eq!(events(&store, &a, TaskEventKind::Unblocked).len(), 1);
}
