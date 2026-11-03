use super::*;
use crate::tracker::{Assignee, TaskState};

fn work_task(store: &Store, project: &str) -> Task {
    store
        .create_tracker_task(
            Task::drafted(project, "Implement the feature", Actor::User, NOW),
            &[],
        )
        .unwrap()
}

fn linked_request(workspace: &str, task_id: &str) -> ReviewOpeningRequest {
    let mut value = serde_json::to_value(request(workspace)).unwrap();
    value["for_task_id"] = serde_json::json!(task_id);
    serde_json::from_value(value).unwrap()
}

#[test]
fn opening_work_task_link_is_atomic_and_survives_retry_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let work = work_task(&store, "/repo");
    let requested = linked_request("ws-1", &work.id);
    let mut opening = store
        .reserve_review_opening("/repo", "open-1", requested.clone())
        .unwrap();
    opening.bindings.push(binding());
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_snapshot BEFORE INSERT ON review_snapshots BEGIN SELECT RAISE(ABORT, 'injected late failure'); END;").unwrap();
    assert!(store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .is_err());
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_snapshot")
        .unwrap();
    let review = store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .unwrap();
    let published = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    assert_eq!(
        published.links.parent_task_id.as_deref(),
        Some(work.id.as_str())
    );
    assert_eq!(
        serde_json::to_value(store.load_review_summary(&review.task_id).unwrap()).unwrap()
            ["parent_task_id"],
        work.id
    );
    assert_eq!(
        serde_json::to_value(store.load_workspace_review_summary("ws-1").unwrap()).unwrap()
            ["parent_task_id"],
        work.id
    );
    let mut finished = work.clone();
    finished.state = TaskState::Closed;
    store
        .save_tracker_task_activity(&finished, &[], &[])
        .unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    let repeated = store
        .reserve_review_opening("/repo", "open-1", requested)
        .unwrap();
    assert_eq!(repeated.task.id, review.task_id);
    assert_eq!(repeated.task.links.parent_task_id, Some(work.id));
    assert_eq!(
        store
            .publish_review_opening("/repo", "open-1", 1, snapshot(), &[])
            .unwrap(),
        review
    );
}

#[test]
fn work_task_link_refuses_foreign_closed_missing_and_review_targets_without_reserving() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let foreign = work_task(&store, "/other");
    let mut closed = work_task(&store, "/repo");
    closed.state = TaskState::Closed;
    store.save_tracker_task_activity(&closed, &[], &[]).unwrap();
    let review = publish(&store);
    for (index, target) in [
        foreign.id.as_str(),
        closed.id.as_str(),
        "missing",
        review.task_id.as_str(),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("refused-{index}");
        assert!(
            store
                .reserve_review_opening(
                    "/repo",
                    &id,
                    linked_request(&format!("workspace-{index}"), target)
                )
                .is_err(),
            "target {target}"
        );
        assert!(store.load_review_opening("/repo", &id).unwrap().is_none());
    }
}

#[test]
fn publication_revalidates_linked_task_after_preparation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut work = work_task(&store, "/repo");
    let mut opening = store
        .reserve_review_opening("/repo", "open-1", linked_request("ws-1", &work.id))
        .unwrap();
    opening.bindings.push(binding());
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    work.state = TaskState::Closed;
    store.save_tracker_task_activity(&work, &[], &[]).unwrap();
    assert!(store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .is_err());
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    assert!(store.load_review(&opening.task.id).unwrap().is_none());
}

#[test]
fn opening_assigns_user_or_named_agent_without_changing_the_work_task() {
    for reviewer in [
        Assignee::User,
        Assignee::Agent {
            agent_id: "agent-1".into(),
        },
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let work = work_task(&store, "/repo");
        let mut asked = linked_request("ws-1", &work.id);
        asked.reviewer = Some(reviewer.clone());
        let opening = store
            .reserve_review_opening("/repo", "open-1", asked)
            .unwrap();
        assert_eq!(opening.task.assignee, Some(reviewer));
        assert_eq!(store.load_tracker_task(&work.id).unwrap(), Some(work));
    }
}

#[test]
fn publication_supersedes_legacy_review_atomically_and_retry_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut work = work_task(&store, "/repo");
    work.status = "in_progress".into();
    work.track("agent-kept").unwrap();
    store.save_tracker_task_activity(&work, &[], &[]).unwrap();
    let mut legacy_snapshot = snapshot();
    legacy_snapshot.id = "legacy-snapshot".into();
    let legacy = store
        .save_review_snapshot(&work.id, "ws-1", 0, legacy_snapshot)
        .unwrap()
        .0;
    let mut opening = store
        .reserve_review_opening("/repo", "replacement", linked_request("ws-1", &work.id))
        .unwrap();
    opening.bindings.push(binding());
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_published_opening BEFORE UPDATE ON review_openings WHEN NEW.state = 'published' BEGIN SELECT RAISE(ABORT, 'injected late failure'); END;").unwrap();
    assert!(store
        .publish_review_opening("/repo", "replacement", opening.version, snapshot(), &[])
        .is_err());
    assert_eq!(store.load_review(&work.id).unwrap().unwrap(), legacy);
    assert_eq!(store.load_tracker_task(&work.id).unwrap().unwrap(), work);
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_published_opening")
        .unwrap();
    let published = store
        .publish_review_opening("/repo", "replacement", opening.version, snapshot(), &[])
        .unwrap();
    let superseded = store.load_review(&work.id).unwrap().unwrap();
    assert_eq!(superseded.state, ReviewState::Completed);
    assert_eq!(superseded.version, legacy.version + 1);
    assert_eq!(
        superseded.completion.as_ref().unwrap().description,
        format!("Superseded by #{}", opening.task.number)
    );
    assert_eq!(superseded.snapshots, legacy.snapshots);
    assert_eq!(
        store.load_tracker_task(&work.id).unwrap().unwrap(),
        work,
        "superseding the review must not finish or untrack its work task"
    );
    let timeline = store.load_tracker_timeline(&work.id).unwrap();
    assert_eq!(
        store
            .publish_review_opening("/repo", "replacement", 1, snapshot(), &[])
            .unwrap(),
        published
    );
    assert_eq!(store.load_review(&work.id).unwrap().unwrap(), superseded);
    assert_eq!(store.load_tracker_timeline(&work.id).unwrap(), timeline);
    assert_eq!(store.load_tracker_task(&work.id).unwrap(), Some(work));
    assert_eq!(timeline.iter().filter(|entry| matches!(entry, crate::tracker::TimelineEntry::Event(event) if event.kind == crate::tracker::TaskEventKind::ReviewCompleted)).count(),1);
}

#[test]
fn publication_preserves_a_legacy_review_that_was_already_completed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let work = work_task(&store, "/repo");
    let mut legacy_snapshot = snapshot();
    legacy_snapshot.id = "legacy-snapshot".into();
    store
        .save_review_snapshot(&work.id, "ws-1", 0, legacy_snapshot)
        .unwrap();
    let completed = store
        .finish_review(&work.id, 1, &Actor::User, "Already checked")
        .unwrap();
    let mut opening = store
        .reserve_review_opening("/repo", "replacement", linked_request("ws-1", &work.id))
        .unwrap();
    opening.bindings.push(binding());
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    let timeline = store.load_tracker_timeline(&work.id).unwrap();
    store
        .publish_review_opening("/repo", "replacement", opening.version, snapshot(), &[])
        .unwrap();
    assert_eq!(store.load_review(&work.id).unwrap().unwrap(), completed);
    assert_eq!(store.load_tracker_timeline(&work.id).unwrap(), timeline);
}
