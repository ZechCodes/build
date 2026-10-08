use super::*;
use crate::tracker::{
    ReviewOpinion, ReviewVerdict, TaskComment, TaskEvent, TaskEventKind, TaskState,
};

fn opinion(task_id: &str, author: Actor, verdict: ReviewVerdict) -> TaskComment {
    TaskComment {
        id: crate::tracker::new_comment_id(),
        task_id: task_id.into(),
        author,
        body: "Review opinion".into(),
        anchor: None,
        reply_to: None,
        opinion: Some(Box::new(ReviewOpinion {
            snapshot_id: "snapshot-1".into(),
            verdict,
        })),
        mentions_user: false,
        notifies_user: false,
        refs: vec![],
        attachments: vec![],
        created_at: NOW.into(),
        author_context: None,
    }
}

fn record(
    store: &Store,
    review: &crate::reviews::records::Review,
    author: Actor,
    verdict: ReviewVerdict,
) {
    let task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    store
        .save_tracker_task_activity(&task, &[opinion(&task.id, author, verdict)], &[])
        .unwrap();
}

fn status(store: &Store, task_id: &str) -> PullRequestStatus {
    store
        .load_review(task_id)
        .unwrap()
        .unwrap()
        .pull_request
        .unwrap()
        .status
}

#[test]
fn opinions_use_latest_per_actor_and_request_changes_wins() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let other = Actor::Agent {
        agent_id: "reviewer".into(),
    };
    record(&store, &review, Actor::User, ReviewVerdict::Approve);
    assert_eq!(status(&store, &review.task_id), PullRequestStatus::Approved);
    record(
        &store,
        &review,
        other.clone(),
        ReviewVerdict::RequestChanges,
    );
    assert_eq!(
        status(&store, &review.task_id),
        PullRequestStatus::ChangesRequested
    );
    assert_eq!(
        store
            .load_tracker_task(&review.task_id)
            .unwrap()
            .unwrap()
            .status,
        "in_progress"
    );
    record(&store, &review, Actor::User, ReviewVerdict::Approve);
    assert_eq!(
        status(&store, &review.task_id),
        PullRequestStatus::ChangesRequested
    );
    record(&store, &review, other, ReviewVerdict::Approve);
    assert_eq!(status(&store, &review.task_id), PullRequestStatus::Approved);
    assert_eq!(
        store
            .load_tracker_task(&review.task_id)
            .unwrap()
            .unwrap()
            .status,
        "in_review"
    );
    assert_eq!(
        store.load_review(&review.task_id).unwrap().unwrap().version,
        review.version + 4
    );
}

#[test]
fn tracker_close_closes_active_pr_and_moves_to_done_atomically() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
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
    let closed = store.load_review(&task.id).unwrap().unwrap();
    assert_eq!(
        closed.pull_request.unwrap().status,
        PullRequestStatus::Closed
    );
    assert_eq!(closed.state, ReviewState::Completed);
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().status,
        "done"
    );
    assert_eq!(closed.completion.unwrap().description, "Task closed");
}

#[test]
fn opinion_on_closed_pr_is_rejected_without_writing_a_comment() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    store
        .complete_review(&review.task_id, review.version, &Actor::User, "Finished")
        .unwrap();
    let task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let before = store.load_tracker_timeline(&task.id).unwrap();
    assert!(store
        .save_tracker_task_activity(
            &task,
            &[opinion(&task.id, Actor::User, ReviewVerdict::Approve)],
            &[]
        )
        .is_err());
    assert_eq!(store.load_tracker_timeline(&task.id).unwrap(), before);
}

#[test]
fn new_snapshot_resets_opinions_and_historical_opinions_do_not_change_status() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    record(&store, &review, Actor::User, ReviewVerdict::Approve);
    let approved = store.load_review(&review.task_id).unwrap().unwrap();
    let mut next = snapshot();
    next.id = "snapshot-2".into();
    let fresh = store
        .save_review_received_snapshot(&review.task_id, approved.version, next, &review.bindings)
        .unwrap();
    assert_eq!(status(&store, &review.task_id), PullRequestStatus::Open);
    record(&store, &review, Actor::User, ReviewVerdict::RequestChanges);
    let after = store.load_review(&review.task_id).unwrap().unwrap();
    assert_eq!(after.pull_request.unwrap().status, PullRequestStatus::Open);
    assert_eq!(after.version, fresh.version);
    assert_eq!(after.snapshots.len(), 2);
}

#[test]
fn failed_opinion_write_leaves_review_task_and_timeline_untouched() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let before = store.load_tracker_timeline(&task.id).unwrap();
    store.fail_next_write();
    assert!(store
        .save_tracker_task_activity(
            &task,
            &[opinion(
                &task.id,
                Actor::User,
                ReviewVerdict::RequestChanges
            )],
            &[]
        )
        .is_err());
    assert_eq!(store.load_review(&task.id).unwrap().unwrap(), review);
    assert_eq!(store.load_tracker_task(&task.id).unwrap().unwrap(), task);
    assert_eq!(store.load_tracker_timeline(&task.id).unwrap(), before);
}

#[test]
fn explicit_opinion_checks_version_and_current_snapshot_and_preserves_history() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let approved = store
        .record_review_opinion(
            &review.task_id,
            review.version,
            &opinion(&review.task_id, Actor::User, ReviewVerdict::Approve),
        )
        .unwrap();
    assert!(matches!(
        store.record_review_opinion(
            &review.task_id,
            review.version,
            &opinion(&review.task_id, Actor::User, ReviewVerdict::RequestChanges)
        ),
        Err(StoreError::ReviewVersionConflict { .. })
    ));
    let mut next = snapshot();
    next.id = "snapshot-2".into();
    let fresh = store
        .save_review_received_snapshot(&review.task_id, approved.version, next, &review.bindings)
        .unwrap();
    assert!(store
        .record_review_opinion(
            &review.task_id,
            fresh.version,
            &opinion(&review.task_id, Actor::User, ReviewVerdict::Approve)
        )
        .is_err());
    assert_eq!(
        store.load_review(&review.task_id).unwrap().unwrap().version,
        fresh.version
    );
}

#[test]
fn ordinary_done_and_tracker_close_do_not_relabel_a_merged_pr() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    store
        .in_transaction(|tx| {
            crate::store::reviews::lifecycle::transition_in_tx(
                tx,
                &review.task_id,
                PullRequestStatus::Merged,
                &Actor::User,
                "Verified merge",
            )
        })
        .unwrap();
    let mut task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    task.status = "in_progress".into();
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from":"done", "to":"in_progress"}),
        NOW,
    );
    store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    assert_eq!(status(&store, &task.id), PullRequestStatus::Merged);
    task.state = TaskState::Closed;
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
    assert_eq!(status(&store, &task.id), PullRequestStatus::Merged);
    task.status = "done".into();
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from":"in_progress", "to":"done"}),
        NOW,
    );
    store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    assert_eq!(status(&store, &task.id), PullRequestStatus::Merged);
}

#[test]
fn merged_pr_cannot_reopen_and_done_never_invents_a_merge() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let mut task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    task.status = "done".into();
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({"from":"in_review", "to":"done"}),
        NOW,
    );
    store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    let closed = store.load_review(&task.id).unwrap().unwrap();
    assert_eq!(
        closed.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Closed
    );
    assert_eq!(
        closed.completion.as_ref().unwrap().description,
        "Marked done"
    );
    let mut fresh = snapshot();
    fresh.id = "reopened-snapshot".into();
    let reopened = store
        .reopen_review_received_snapshot(
            &task.id,
            closed.version,
            fresh,
            &review.bindings,
            &Actor::User,
        )
        .unwrap();
    let (merged, _) = store
        .in_transaction(|tx| {
            crate::store::reviews::lifecycle::transition_in_tx(
                tx,
                &task.id,
                PullRequestStatus::Merged,
                &Actor::User,
                "Verified merge",
            )
        })
        .unwrap();
    let mut refused = snapshot();
    refused.id = "refused-snapshot".into();
    assert!(store
        .reopen_review_received_snapshot(
            &task.id,
            merged.version,
            refused,
            &reopened.bindings,
            &Actor::User
        )
        .is_err());
    assert_eq!(store.load_review(&task.id).unwrap().unwrap(), merged);
}

#[test]
fn stale_ordinary_comment_preserves_concurrent_merge_and_tracker_closure() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let stale = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    store
        .in_transaction(|tx| {
            crate::store::reviews::lifecycle::transition_in_tx(
                tx,
                &review.task_id,
                PullRequestStatus::Merged,
                &Actor::User,
                "Verified merge",
            )
        })
        .unwrap();
    let mut closed = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    closed.state = TaskState::Closed;
    closed.closed_at = Some(NOW.into());
    let event = TaskEvent::new(
        &closed.id,
        Actor::User,
        TaskEventKind::Closed,
        serde_json::json!({}),
        NOW,
    );
    store
        .save_tracker_task_activity(&closed, &[], &[event])
        .unwrap();
    let before = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let mut comment = opinion(&review.task_id, Actor::User, ReviewVerdict::Approve);
    comment.opinion = None;
    store
        .save_tracker_task_activity(&stale, &[comment], &[])
        .unwrap();
    let after = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.done_at, before.done_at);
    assert_eq!(after.state, before.state);
    assert_eq!(after.closed_at, before.closed_at);
    assert_eq!(status(&store, &review.task_id), PullRequestStatus::Merged);
}

#[test]
fn running_merge_blocks_history_deletion_before_any_pin_release() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let intent = store
        .reserve_review_merge("/repo", "merge-1", merge_request(&review))
        .unwrap();
    let releases = std::cell::Cell::new(0);
    assert!(store
        .delete_tracker_tasks_of_project("/repo", |_| {
            releases.set(releases.get() + 1);
            Ok(())
        })
        .is_err());
    assert_eq!(releases.get(), 0);
    assert_eq!(store.load_review(&review.task_id).unwrap().unwrap(), review);
    assert_eq!(
        store.load_review_merge_intent("/repo", "merge-1").unwrap(),
        Some(intent)
    );
    let other = store
        .create_tracker_task(Task::drafted("/other", "Other work", Actor::User, NOW), &[])
        .unwrap();
    store
        .delete_tracker_tasks_of_project("/other", |_| Ok(()))
        .unwrap();
    assert!(store.load_tracker_task(&other.id).unwrap().is_none());
}

#[test]
fn stale_read_backfill_preserves_merge_lifecycle_and_other_live_fields() {
    let home = tempfile::tempdir().unwrap();
    let store = Store::new(home.path()).unwrap();
    let review = publish(&store);
    let mut stale = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let identity = crate::tracker::TaskAgentIdentity {
        agent_id: "reviewer".into(), name: Some("Reviewer".into()), ordinal: Some(1),
        workspace_id: None, workspace_name: None, provider: None, available: true,
    };
    stale.identities.insert("reviewer".into(), identity.clone());
    stale.done_at = Some("2000-01-01T00:00:00Z".into());
    stale.title = "Stale title".into();
    store.in_transaction(|tx| crate::store::reviews::lifecycle::transition_in_tx(tx, &review.task_id, PullRequestStatus::Merged, &Actor::User, "Verified merge")).unwrap();
    let before = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let migrated = store.backfill_tracker_task(&stale).unwrap();
    assert_eq!(migrated.status, before.status);
    assert_eq!(migrated.done_at, before.done_at);
    assert_eq!(migrated.state, before.state);
    assert_eq!(migrated.title, before.title);
    assert_eq!(migrated.updated_at, before.updated_at);
    assert_eq!(migrated.identities.get("reviewer"), Some(&identity));
    assert_eq!(store.load_tracker_task(&review.task_id).unwrap().unwrap(), migrated);
}
