use super::*;
use crate::reviews::model::ReviewSnapshot;
use crate::reviews::records::ReviewState;
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind, DONE_STATUS};

const NOW: &str = "2026-10-02T12:00:00Z";

fn task(store: &Store) -> Task {
    let draft = Task::drafted("/repo", "review me", Actor::User, NOW);
    store.create_tracker_task(draft, &[]).unwrap()
}

fn snapshot(id: &str) -> ReviewSnapshot {
    ReviewSnapshot {
        id: id.into(),
        number: 0,
        created_at: NOW.into(),
        author: Actor::User,
        directories: Vec::new(),
    }
}

#[test]
fn snapshots_append_and_compare_versions_without_losing_the_winner() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    assert!(store.load_review(&task.id).unwrap().is_none());
    let first = store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    assert_eq!((first.version, first.snapshots[0].number), (1, 1));

    let second = store
        .save_review_snapshot(&task.id, "ws-1", 1, snapshot("rs-second"))
        .unwrap();
    assert_eq!((second.version, second.snapshots[1].number), (2, 2));
    assert!(matches!(
        store.save_review_snapshot(&task.id, "ws-1", 1, snapshot("rs-loser")),
        Err(StoreError::ReviewVersionConflict { .. })
    ));
    assert!(matches!(
        store.save_review_snapshot(&task.id, "ws-2", 2, snapshot("rs-other-workspace")),
        Err(StoreError::ReviewWorkspaceMismatch { .. })
    ));
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(reopened.load_review(&task.id).unwrap().unwrap(), second);
}

#[test]
fn completion_writes_review_task_and_timeline_in_one_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    let actor = Actor::Agent {
        agent_id: "agent-1".into(),
    };
    let review = store
        .complete_review(&task.id, 1, &actor, "merged API to dev")
        .unwrap();
    assert_eq!(review.version, 2);
    assert_eq!(review.state, ReviewState::Completed);
    assert_eq!(review.completion.as_ref().unwrap().actor, actor);
    assert_eq!(
        review.completion.as_ref().unwrap().description,
        "merged API to dev"
    );
    let task = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, DONE_STATUS);
    assert_eq!(task.state, crate::tracker::TaskState::Open);
    assert!(task.done_at.is_some());
    let events: Vec<TaskEvent> = store
        .load_tracker_timeline(&task.id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| match entry {
            crate::tracker::TimelineEntry::Event(event) => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TaskEventKind::ReviewCompleted);
    assert_eq!(events[0].payload["description"], "merged API to dev");
    assert_eq!(events[0].payload["snapshot_id"], "rs-first");
    assert!(matches!(
        store.complete_review(&task.id, 1, &Actor::User, "again"),
        Err(StoreError::ReviewVersionConflict { .. })
    ));
}

#[test]
fn failed_completion_leaves_every_record_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    let before = store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    store.fail_next_write();
    assert!(store
        .complete_review(&task.id, 1, &Actor::User, "used own tools")
        .is_err());
    assert_eq!(store.load_review(&task.id).unwrap().unwrap(), before);
    assert_eq!(store.load_tracker_task(&task.id).unwrap().unwrap(), task);
    assert!(store.load_tracker_timeline(&task.id).unwrap().is_empty());
}

#[test]
fn fresh_snapshot_reopens_review_without_moving_or_reopening_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    store
        .complete_review(&task.id, 1, &Actor::User, "used own tools")
        .unwrap();
    let task_before = store.load_tracker_task(&task.id).unwrap().unwrap();
    let review = store
        .save_review_snapshot(&task.id, "ws-1", 2, snapshot("rs-second"))
        .unwrap();
    assert_eq!(review.version, 3);
    assert_eq!(review.state, ReviewState::Open);
    assert!(review.completion.is_none());
    assert_eq!(review.snapshots.len(), 2);
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap(),
        task_before
    );
    assert_eq!(store.load_tracker_timeline(&task.id).unwrap().len(), 1);
}

#[test]
fn done_move_uses_review_state_inside_the_write_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    store
        .complete_review(&task.id, 1, &Actor::User, "first completion")
        .unwrap();

    let mut candidate = store.load_tracker_task(&task.id).unwrap().unwrap();
    candidate.status = "in_review".into();
    candidate.done_at = None;
    store
        .save_tracker_task_activity(&candidate, &[], &[])
        .unwrap();
    candidate.status = DONE_STATUS.into();
    candidate.body = "kept as an ordinary edit".into();
    let moved = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from": "in_review", "to": "done"}),
        NOW,
    );
    let completion = store
        .complete_review_with_task_activity(&candidate, &[], &[moved], &Actor::User, NOW)
        .unwrap();
    assert!(
        completion.is_none(),
        "an already completed review stays completed"
    );
    assert_eq!(store.load_review(&task.id).unwrap().unwrap().version, 2);
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().body,
        "kept as an ordinary edit"
    );

    // A snapshot can reopen the review after an earlier app read. The same
    // Done writer now sees Open and finishes the newest snapshot.
    store
        .save_review_snapshot(&task.id, "ws-1", 2, snapshot("rs-second"))
        .unwrap();
    candidate.status = "in_review".into();
    candidate.done_at = None;
    store
        .save_tracker_task_activity(&candidate, &[], &[])
        .unwrap();
    candidate.status = DONE_STATUS.into();
    let moved = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from": "in_review", "to": "done"}),
        NOW,
    );
    let completion = store
        .complete_review_with_task_activity(&candidate, &[], &[moved], &Actor::User, NOW)
        .unwrap();
    assert_eq!(completion.unwrap().payload["snapshot_id"], "rs-second");
    let review = store.load_review(&task.id).unwrap().unwrap();
    assert_eq!(review.version, 4);
    assert_eq!(review.state, ReviewState::Completed);
}

#[test]
fn completion_requires_a_brief_action_description() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    assert!(matches!(
        store.complete_review(&task.id, 1, &Actor::User, "  "),
        Err(StoreError::ReviewDescriptionInvalid)
    ));
    assert_eq!(store.load_review(&task.id).unwrap().unwrap().version, 1);
}

#[test]
fn explicit_project_history_deletion_removes_reviews_with_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = task(&store);
    store
        .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
        .unwrap();
    assert_eq!(store.load_reviews_of_project("/repo").unwrap().len(), 1);
    store
        .delete_tracker_tasks_of_project("/repo", |_| Ok(()))
        .unwrap();
    assert!(store.load_review(&task.id).unwrap().is_none());
    assert!(store.load_reviews_of_project("/repo").unwrap().is_empty());
    let count: i64 = store
        .connection()
        .query_row("SELECT COUNT(*) FROM review_snapshots", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn store_completion_description_boundary_counts_utf8_bytes() {
    for (description, accepted) in [
        ("a".repeat(2_000), true),
        ("a".repeat(2_001), false),
        (format!(" {} ", "a".repeat(2_000)), true),
        (" \t ".into(), false),
        ("é".repeat(1_000), true),
        ("é".repeat(1_001), false),
        ("🦀".repeat(500), true),
        ("🦀".repeat(501), false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task = task(&store);
        store
            .save_review_snapshot(&task.id, "ws-1", 0, snapshot("rs-first"))
            .unwrap();
        let result = store.complete_review(&task.id, 1, &Actor::User, &description);
        assert_eq!(
            result.is_ok(),
            accepted,
            "{} UTF-8 bytes",
            description.len()
        );
        if !accepted {
            assert!(matches!(result, Err(StoreError::ReviewDescriptionInvalid)));
        }
    }
}
