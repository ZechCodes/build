use super::*;
use crate::reviews::records::Review;
use crate::tracker::{TaskEvent, TaskEventKind, TaskState};

fn publish_linked(store: &Store, work: &Task, index: usize) -> Review {
    let mut asked = request(&format!("workspace-{index}"));
    asked.for_task_id = Some(work.id.clone());
    let id = format!("linked-{index}");
    let mut opening = store
        .reserve_review_opening(&work.project_path, &id, asked)
        .unwrap();
    let mut bound = binding();
    bound.dedicated_branch_ref = format!("refs/heads/review/{index}");
    bound.receiving_ref = bound.dedicated_branch_ref.clone();
    opening.bindings.push(bound);
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    let mut captured = snapshot();
    captured.id = format!("snapshot-{index}");
    store
        .publish_review_opening(&work.project_path, &id, opening.version, captured, &[])
        .unwrap()
}

#[test]
fn linked_review_detail_projection_is_bounded_newest_first_and_project_scoped() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let work = store
        .create_tracker_task(Task::drafted("/repo", "Work", Actor::User, NOW), &[])
        .unwrap();
    assert_eq!(
        store
            .load_linked_review_summaries("/repo", &work.id)
            .unwrap(),
        (vec![], false)
    );
    let mut numbers = Vec::new();
    for index in 0..22 {
        let review = publish_linked(&store, &work, index);
        numbers.push(
            store
                .load_tracker_task(&review.task_id)
                .unwrap()
                .unwrap()
                .number,
        );
    }
    let mut ordinary = Task::drafted("/repo", "Ordinary child", Actor::User, NOW);
    ordinary.links.parent_task_id = Some(work.id.clone());
    store.create_tracker_task(ordinary, &[]).unwrap();
    let (summaries, truncated) = store
        .load_linked_review_summaries("/repo", &work.id)
        .unwrap();
    assert!(truncated);
    assert_eq!(summaries.len(), 20);
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.number.unwrap())
            .collect::<Vec<_>>(),
        numbers.into_iter().rev().take(20).collect::<Vec<_>>()
    );
    assert!(summaries
        .iter()
        .all(|summary| summary.parent_task_id.as_deref() == Some(work.id.as_str())));
    assert_eq!(
        store
            .load_linked_review_summaries("/other", &work.id)
            .unwrap(),
        (vec![], false)
    );
    assert_eq!(
        store
            .load_linked_review_summaries("/repo", "missing")
            .unwrap(),
        (vec![], false)
    );
}

#[test]
fn linked_review_projection_keeps_terminal_reviews_and_detects_exact_limit() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let work = store
        .create_tracker_task(Task::drafted("/repo", "Work", Actor::User, NOW), &[])
        .unwrap();
    for index in 0..20 {
        publish_linked(&store, &work, index);
    }
    let (before, truncated) = store
        .load_linked_review_summaries("/repo", &work.id)
        .unwrap();
    assert!(!truncated);
    let review = &before[0];
    let mut task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    task.state = TaskState::Closed;
    task.closed_at = Some(NOW.into());
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Closed,
        serde_json::json!({}),
        NOW,
    );
    store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    let (after, truncated) = store
        .load_linked_review_summaries("/repo", &work.id)
        .unwrap();
    assert!(!truncated);
    assert_eq!(after.len(), 20);
    assert_eq!(after[0].status, PullRequestStatus::Closed);
    assert_eq!(after[0].task_id, review.task_id);
}

#[test]
fn review_summary_parent_links_follow_the_current_task_after_relinking() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let original = store
        .create_tracker_task(Task::drafted("/repo", "Original", Actor::User, NOW), &[])
        .unwrap();
    let replacement = store
        .create_tracker_task(Task::drafted("/repo", "Replacement", Actor::User, NOW), &[])
        .unwrap();
    let review = publish_linked(&store, &original, 0);
    let mut task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    task.links.parent_task_id = Some(replacement.id.clone());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let summary = store.load_review_summary(&review.task_id).unwrap().unwrap();
    assert_eq!(
        summary.parent_task_id.as_deref(),
        Some(replacement.id.as_str())
    );
    let workspace = store
        .load_workspace_review_summary("workspace-0")
        .unwrap()
        .unwrap();
    assert_eq!(workspace.parent_task_id, summary.parent_task_id);
    assert!(store
        .load_linked_review_summaries("/repo", &original.id)
        .unwrap()
        .0
        .is_empty());
    assert_eq!(
        store
            .load_linked_review_summaries("/repo", &replacement.id)
            .unwrap()
            .0[0]
            .parent_task_id,
        Some(replacement.id)
    );
}
