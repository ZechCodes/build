use super::*;
use crate::reviews::model::*;
use crate::reviews::records::ReviewState;
use crate::tracker::{Actor, Task};
use std::collections::BTreeMap;

mod lifecycle;

const NOW: &str = "2026-10-08T00:00:00Z";

fn request(workspace: &str) -> ReviewOpeningRequest {
    ReviewOpeningRequest {
        workspace_id: workspace.into(),
        title: "Review feature".into(),
        description: "Committed work".into(),
        creator: Actor::User,
        reviewer: None,
        directories: vec![ReviewMembership {
            directory_id: "dir-1".into(),
            source_id: "source-1".into(),
            kind: ReviewMembershipKind::Git,
            reason: None,
        }],
        base_branches: BTreeMap::from([("dir-1".into(), "refs/heads/main".into())]),
    }
}

fn binding() -> ReviewBranchBinding {
    ReviewBranchBinding {
        directory_id: "dir-1".into(),
        source_id: "source-1".into(),
        repository_id: "repo-1".into(),
        working_repository: "/work/Build".into(),
        source_repository: "/source/Build".into(),
        original_branch_ref: Some("refs/heads/build/feature".into()),
        initial_head: "abc123".into(),
        dedicated_branch_ref: "refs/heads/review/1-feature".into(),
        base_branch_ref: "refs/heads/main".into(),
        receiving_repository: "/receivers/repo-1.git".into(),
        receiving_ref: "refs/heads/review/1-feature".into(),
        remote_name: "build-review".into(),
        last_received_head: Some("abc123".into()),
        preparation: ReviewPreparationState::Ready,
        publication: ReviewPublicationState::Published,
        recovery: None,
    }
}

fn snapshot() -> ReviewSnapshot {
    ReviewSnapshot {
        publication: None,
        id: "snapshot-1".into(),
        number: 0,
        author: Actor::User,
        created_at: NOW.into(),
        directories: vec![ReviewDirectory {
            id: "dir-1".into(),
            source_id: "source-1".into(),
            name: "Build".into(),
            path: "/work/Build".into(),
            source_path: "/source/Build".into(),
            is_git: true,
            status: ReviewDirectoryStatus::Git,
            reason: None,
            common_git_dir: Some("/receivers/repo-1.git".into()),
            branch: Some("review/1-feature".into()),
            base: Some(ReviewBase {
                kind: ReviewBaseKind::Configured,
                name: Some("main".into()),
                oid: "base123".into(),
            }),
            head: Some("abc123".into()),
            uncommitted_files: None,
        }],
    }
}

fn prepare(store: &Store, workspace: &str, request_id: &str) -> ReviewOpening {
    let mut opening = store
        .reserve_review_opening("/repo", request_id, request(workspace))
        .unwrap();
    opening.bindings.push(binding());
    store
        .save_review_opening(&opening, opening.version)
        .unwrap()
}

fn publish(store: &Store) -> crate::reviews::records::Review {
    let opening = prepare(store, "ws-1", "open-1");
    store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .unwrap()
}

#[test]
fn reservation_survives_restart_and_normal_tasks_skip_its_number() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let opening = store
        .reserve_review_opening("/repo", "open-1", request("ws-1"))
        .unwrap();
    assert_eq!(opening.task.number, 1);
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store.load_review_opening("/repo", "open-1").unwrap(),
        Some(opening.clone())
    );
    let task = store
        .create_tracker_task(Task::drafted("/repo", "normal", Actor::User, NOW), &[])
        .unwrap();
    assert_eq!(task.number, 2);
    assert_eq!(
        store
            .reserve_review_opening("/repo", "open-1", request("ws-1"))
            .unwrap(),
        opening
    );
    let mut changed = request("ws-1");
    changed.title = "different".into();
    assert!(matches!(
        store.reserve_review_opening("/repo", "open-1", changed),
        Err(StoreError::ReviewRequestConflict { .. })
    ));
    // The same request ID is scoped to its canonical project, not globally.
    let other = store
        .reserve_review_opening("/other", "open-1", request("ws-2"))
        .unwrap();
    assert_eq!(other.task.number, 1);
}

#[test]
fn independent_connections_cannot_claim_the_same_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let left = Store::new(dir.path()).unwrap();
    let right = Store::new(dir.path()).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let run = |store: Store, id: &'static str, barrier: std::sync::Arc<std::sync::Barrier>| {
        std::thread::spawn(move || {
            barrier.wait();
            store.reserve_review_opening("/repo", id, request("ws-1"))
        })
    };
    let a = run(left, "a", barrier.clone());
    let b = run(right, "b", barrier);
    let results = [a.join().unwrap(), b.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(results
        .iter()
        .any(|result| matches!(result, Err(StoreError::ReviewWorkspaceBusy { .. }))));
}

#[test]
fn opening_publication_is_atomic_idempotent_and_keeps_the_claim() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let opening = prepare(&store, "ws-1", "open-1");
    store.fail_next_write();
    assert!(store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .is_err());
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    assert!(store.load_review(&opening.task.id).unwrap().is_none());
    let review = store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .unwrap();
    assert_eq!(review.mode, ReviewMode::PullRequest);
    assert_eq!(
        review
            .pull_request
            .as_ref()
            .unwrap()
            .latest_published_snapshot_id
            .as_deref(),
        Some("snapshot-1")
    );
    assert_eq!(review.bindings, opening.bindings);
    assert_eq!(
        store
            .load_tracker_task(&opening.task.id)
            .unwrap()
            .unwrap()
            .status,
        "in_review"
    );
    assert_eq!(
        store
            .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
            .unwrap(),
        review
    );
    assert!(matches!(
        store.reserve_review_opening("/repo", "open-2", request("ws-1")),
        Err(StoreError::ReviewWorkspaceBusy { .. })
    ));
    assert_eq!(
        store.load_workspace_review_summary("ws-1").unwrap(),
        review.summary()
    );
}

#[test]
fn journal_cas_and_identity_checks_preserve_recovery_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut opening = prepare(&store, "ws-1", "open-1");
    let original = opening.clone();
    opening.state = ReviewOpeningState::Interrupted;
    let interrupted = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    assert!(matches!(
        store.save_review_opening(&opening, opening.version),
        Err(StoreError::ReviewOperationVersionConflict { .. })
    ));
    let mut changed = interrupted.clone();
    changed.bindings[0].receiving_repository = "/untrusted.git".into();
    assert!(matches!(
        store.save_review_opening(&changed, interrupted.version),
        Err(StoreError::ReviewPullRequestInvalid(_))
    ));
    assert!(matches!(
        store.reserve_review_opening("/repo", "open-2", request("ws-1")),
        Err(StoreError::ReviewWorkspaceBusy { .. })
    ));
    let mut cancelled = interrupted;
    cancelled.state = ReviewOpeningState::Cancelled;
    store
        .save_review_opening(&cancelled, cancelled.version)
        .unwrap();
    let next = store
        .reserve_review_opening("/repo", "open-2", request("ws-1"))
        .unwrap();
    assert_eq!(next.task.number, original.task.number + 1);
}

#[test]
fn sync_revision_is_separate_from_review_version_and_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let observation = ReviewSyncObservation {
        task_id: review.task_id.clone(),
        directory_id: "dir-1".into(),
        revision: 0,
        target_head: None,
        comparison_base: None,
        health: ReviewSyncHealth::Unavailable,
        working_head: Some("def456".into()),
        received_head: None,
        snapshot_head: Some("abc123".into()),
        pending_commits: None,
        observed_at: NOW.into(),
        error: Some("receiver missing".into()),
    };
    let saved = store.save_review_sync_observation(&observation, 0).unwrap();
    assert_eq!(saved.revision, 1);
    assert!(matches!(
        store.save_review_sync_observation(&observation, 0),
        Err(StoreError::ReviewOperationVersionConflict { .. })
    ));
    assert_eq!(
        store.load_review(&review.task_id).unwrap(),
        Some(review.clone())
    );
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store
            .load_review_sync_observations(&review.task_id)
            .unwrap(),
        vec![saved]
    );
}

#[test]
fn legacy_completion_preserves_pr_mode_and_releases_claim_as_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    assert!(matches!(
        store.save_review_snapshot(&review.task_id, "ws-other", review.version, snapshot()),
        Err(StoreError::ReviewPullRequestInvalid(_))
    ));
    let completed = store
        .complete_review(&review.task_id, review.version, &Actor::User, "Marked done")
        .unwrap();
    assert_eq!(completed.state, ReviewState::Completed);
    assert_eq!(
        completed.pull_request.unwrap().status,
        PullRequestStatus::Closed
    );
    assert!(store
        .reserve_review_opening("/repo", "open-2", request("ws-1"))
        .is_ok());
}

fn merge_request(review: &crate::reviews::records::Review) -> ReviewMergeRequest {
    ReviewMergeRequest {
        task_id: review.task_id.clone(),
        expected_version: review.version,
        snapshot_id: "snapshot-1".into(),
        actor: Actor::User,
        sources: vec![ReviewMergeSource {
            directory_id: "dir-1".into(),
            repository_id: "repo-1".into(),
            base_branch_ref: "refs/heads/main".into(),
            head: "abc123".into(),
            expected_base_head: "base123".into(),
            push: Some(ReviewMergePush {
                remote: "origin".into(),
                branch: "main".into(),
            }),
        }],
    }
}

#[test]
fn merge_intents_survive_restart_and_match_requests_before_version_checks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let request = merge_request(&review);
    let intent = store
        .reserve_review_merge("/repo", "merge-1", request.clone())
        .unwrap();
    assert!(store
        .reserve_review_merge("/other", "merge-1", request.clone())
        .is_err());
    assert!(store
        .reserve_review_merge("/repo", "merge-2", request.clone())
        .is_err());
    let mut result = intent.clone();
    result.state = ReviewMergeState::Interrupted;
    result.action_ids.push("action-1".into());
    let saved = store
        .save_review_merge_intent(&result, result.version)
        .unwrap();
    assert!(matches!(
        store.save_review_merge_intent(&result, result.version),
        Err(StoreError::ReviewOperationVersionConflict { .. })
    ));
    store
        .complete_review(&review.task_id, review.version, &Actor::User, "Closed")
        .unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store.load_review_merge_intent("/repo", "merge-1").unwrap(),
        Some(saved.clone())
    );
    assert_eq!(
        store
            .reserve_review_merge("/repo", "merge-1", request.clone())
            .unwrap(),
        saved
    );
    let mut changed = request;
    changed.sources[0].base_branch_ref = "refs/heads/other".into();
    assert!(matches!(
        store.reserve_review_merge("/repo", "merge-1", changed),
        Err(StoreError::ReviewRequestConflict { .. })
    ));
}

#[test]
fn merge_intent_discovery_keeps_finished_requests_and_filters_by_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let request = merge_request(&review);
    let mut failed = store
        .reserve_review_merge("/repo", "merge-first", request.clone())
        .unwrap();
    failed.state = ReviewMergeState::Failed;
    let failed = store
        .save_review_merge_intent(&failed, failed.version)
        .unwrap();
    let running = store
        .reserve_review_merge("/repo", "merge-second", request)
        .unwrap();
    assert_eq!(
        store.load_review_merge_intents(&review.task_id).unwrap(),
        vec![failed.clone(), running.clone()]
    );
    assert!(store
        .load_review_merge_intents("other-task")
        .unwrap()
        .is_empty());
    drop(store);
    let restored = Store::new(dir.path()).unwrap();
    assert_eq!(
        restored.load_review_merge_intents(&review.task_id).unwrap(),
        vec![failed, running]
    );
}

#[test]
fn merge_admission_requires_exact_published_binding_vector() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let request = merge_request(&review);
    for field in 0..5 {
        let mut invalid = request.clone();
        match field {
            0 => invalid.sources.clear(),
            1 => invalid.sources[0].head = "unpublished".into(),
            2 => invalid.sources[0].repository_id = "different".into(),
            3 => invalid.sources[0].base_branch_ref = "refs/heads/other".into(),
            _ => invalid.sources.push(invalid.sources[0].clone()),
        }
        assert!(store
            .reserve_review_merge("/repo", "invalid", invalid)
            .is_err());
    }
    assert!(store
        .load_review_merge_intent("/repo", "invalid")
        .unwrap()
        .is_none());
    assert!(store
        .reserve_review_merge("/repo", "valid", request)
        .is_ok());
}

#[test]
fn upgrading_real_v14_shape_preserves_legacy_header_and_snapshot_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(Task::drafted("/repo", "legacy", Actor::User, NOW), &[])
        .unwrap();
    let legacy = format!(
        r#"{{"task_id":"{}","workspace_id":"ws-old","version":1,"state":"open","completion":null}}"#,
        task.id
    );
    let mut old = snapshot();
    old.number = 1;
    let old_snapshot = serde_json::to_string(&old).unwrap();
    {
        let conn = store.connection();
        conn.execute(
            "INSERT INTO reviews VALUES (?1, 'ws-old', 1, ?2)",
            rusqlite::params![task.id, legacy],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO review_snapshots VALUES ('snapshot-1', ?1, 1, ?2)",
            rusqlite::params![task.id, old_snapshot],
        )
        .unwrap();
        // Recreate the pre-PR schema, including the rest of the real v14 store.
        conn.execute_batch("DROP TABLE review_workspace_claims; DROP TABLE review_openings; DROP TABLE review_branch_bindings; DROP TABLE review_sync_observations; DROP TABLE review_merge_intents; UPDATE meta SET value = '14' WHERE key = 'schema_version';").unwrap();
    }
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(SCHEMA_VERSION, 15);
    let review = store.load_review(&task.id).unwrap().unwrap();
    assert_eq!(review.mode, ReviewMode::Snapshot);
    assert!(review.pull_request.is_none());
    assert!(review.bindings.is_empty());
    let conn = store.connection();
    let raw: String = conn
        .query_row(
            "SELECT record FROM reviews WHERE task_id = ?1",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, legacy);
    let raw: String = conn
        .query_row(
            "SELECT record FROM review_snapshots WHERE task_id = ?1",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, old_snapshot);
    drop(conn);
    let complete = store
        .complete_review(&task.id, 1, &Actor::User, "legacy complete")
        .unwrap();
    let raw = serde_json::to_value(complete).unwrap();
    assert!(raw.get("mode").is_none());
    assert!(raw.get("pull_request").is_none());
    assert!(raw.get("bindings").is_none());
}

#[test]
fn explicit_history_deletion_cleans_cancelled_and_published_pr_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let mut merge = store
        .reserve_review_merge("/repo", "merge-1", merge_request(&review))
        .unwrap();
    // History deletion waits for admitted work to finish or be recovered.
    // No Git was started by this store-only fixture.
    merge.state = ReviewMergeState::Interrupted;
    store
        .save_review_merge_intent(&merge, merge.version)
        .unwrap();
    let mut cancelled = store
        .reserve_review_opening("/repo", "open-2", request("ws-2"))
        .unwrap();
    cancelled.state = ReviewOpeningState::Cancelled;
    store
        .save_review_opening(&cancelled, cancelled.version)
        .unwrap();
    store
        .delete_tracker_tasks_of_project("/repo", |_| Ok(()))
        .unwrap();
    assert!(store
        .load_review_opening("/repo", "open-2")
        .unwrap()
        .is_none());
    assert!(store
        .load_review_merge_intent("/repo", "merge-1")
        .unwrap()
        .is_none());
    for workspace in ["ws-1", "ws-2"] {
        assert!(store
            .reserve_review_opening("/repo", workspace, request(workspace))
            .is_ok());
    }
}

#[test]
fn history_deletion_preserves_unfinished_opening_recovery_across_restart() {
    for state in [
        ReviewOpeningState::Interrupted,
        ReviewOpeningState::Preparing,
        ReviewOpeningState::Failed,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let published = publish(&store);
        let mut opening = store
            .reserve_review_opening("/repo", "open-2", request("ws-2"))
            .unwrap();
        let mut prepared = binding();
        prepared.dedicated_branch_ref = "refs/heads/review/2-preparing".into();
        prepared.receiving_ref = prepared.dedicated_branch_ref.clone();
        prepared.preparation = ReviewPreparationState::RemoteConfigured;
        prepared.publication = ReviewPublicationState::Interrupted;
        prepared.last_received_head = None;
        prepared.recovery = Some("Inspect the prepared remote before unwinding".into());
        opening.bindings.push(prepared.clone());
        opening.state = state;
        let opening = store
            .save_review_opening(&opening, opening.version)
            .unwrap();

        let released = std::cell::Cell::new(false);
        let result = store.delete_tracker_tasks_of_project("/repo", |_| {
            released.set(true);
            Ok(())
        });
        assert!(matches!(
            result,
            Err(StoreError::ReviewPullRequestInvalid(_))
        ));
        assert!(
            !released.get(),
            "refusal must precede the Git release callback"
        );
        drop(store);

        let store = Store::new(dir.path()).unwrap();
        assert_eq!(
            store.load_review_opening("/repo", "open-2").unwrap(),
            Some(opening.clone())
        );
        assert_eq!(
            store.load_unfinished_review_openings().unwrap(),
            vec![opening.clone()]
        );
        let raw: String = store.connection().query_row(
            "SELECT record FROM review_branch_bindings WHERE task_id = ?1 AND directory_id = ?2",
            rusqlite::params![opening.task.id, prepared.directory_id], |row| row.get(0),
        ).unwrap();
        assert_eq!(
            serde_json::from_str::<ReviewBranchBinding>(&raw).unwrap(),
            prepared
        );
        assert_eq!(
            store.load_review(&published.task_id).unwrap(),
            Some(published)
        );
        assert_eq!(
            store
                .reserve_review_opening("/repo", "open-2", request("ws-2"))
                .unwrap(),
            opening
        );
        assert!(matches!(
            store.reserve_review_opening("/repo", "another", request("ws-2")),
            Err(StoreError::ReviewWorkspaceBusy { .. })
        ));
        let normal = store
            .create_tracker_task(
                Task::drafted("/repo", "after restart", Actor::User, NOW),
                &[],
            )
            .unwrap();
        assert_eq!(normal.number, opening.task.number + 1);
    }
}

#[test]
fn history_deletion_checks_unfinished_openings_only_in_its_project() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let published = publish(&store);
    let other = store
        .reserve_review_opening("/other", "open-other", request("ws-other"))
        .unwrap();
    let released = std::cell::Cell::new(false);
    store
        .delete_tracker_tasks_of_project("/repo", |_| {
            released.set(true);
            Ok(())
        })
        .unwrap();
    assert!(released.get());
    assert!(store.load_review(&published.task_id).unwrap().is_none());
    assert_eq!(
        store.load_review_opening("/other", "open-other").unwrap(),
        Some(other)
    );
    assert!(matches!(
        store.reserve_review_opening("/other", "another", request("ws-other")),
        Err(StoreError::ReviewWorkspaceBusy { .. })
    ));
}

#[test]
fn preparation_reserves_exact_branch_identity_before_git_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let first = prepare(&store, "ws-1", "open-1");
    let mut second = store
        .reserve_review_opening("/other", "open-2", request("ws-2"))
        .unwrap();
    second.bindings.push(binding());
    assert!(store.save_review_opening(&second, second.version).is_err());
    // Cancellation is the service's assertion that owned refs were unwound.
    let mut cancelled = first;
    cancelled.state = ReviewOpeningState::Cancelled;
    store
        .save_review_opening(&cancelled, cancelled.version)
        .unwrap();
    assert!(store.save_review_opening(&second, second.version).is_ok());
}

#[test]
fn independent_openings_reserve_distinct_numbers_and_are_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let stores = [
        Store::new(dir.path()).unwrap(),
        Store::new(dir.path()).unwrap(),
    ];
    let handles: Vec<_> = stores
        .into_iter()
        .enumerate()
        .map(|(index, store)| {
            std::thread::spawn(move || {
                store
                    .reserve_review_opening(
                        "/repo",
                        &format!("request-{index}"),
                        request(&format!("workspace-{index}")),
                    )
                    .unwrap()
            })
        })
        .collect();
    let mut numbers: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().task.number)
        .collect();
    numbers.sort();
    assert_eq!(numbers, [1, 2]);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(store.load_unfinished_review_openings().unwrap().len(), 2);
}

#[test]
fn receiver_ref_is_reserved_independently_of_working_branch_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    prepare(&store, "ws-1", "open-1");
    let mut other = store
        .reserve_review_opening("/repo", "open-2", request("ws-2"))
        .unwrap();
    let mut colliding = binding();
    colliding.dedicated_branch_ref = "refs/heads/review/2-other".into();
    other.bindings.push(colliding);
    assert!(store.save_review_opening(&other, other.version).is_err());
}

#[test]
fn late_sql_failure_rolls_back_every_publication_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let opening = prepare(&store, "ws-1", "open-1");
    store.connection().execute_batch("CREATE TRIGGER reject_snapshot BEFORE INSERT ON review_snapshots BEGIN SELECT RAISE(ABORT, 'injected late failure'); END;").unwrap();
    let event = crate::tracker::TaskEvent::new(
        &opening.task.id,
        Actor::User,
        crate::tracker::TaskEventKind::Created,
        serde_json::json!({}),
        NOW,
    );
    assert!(store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[event])
        .is_err());
    assert!(store.load_review(&opening.task.id).unwrap().is_none());
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    assert_eq!(
        store.load_review_opening("/repo", "open-1").unwrap(),
        Some(opening.clone())
    );
    assert!(store
        .load_tracker_timeline(&opening.task.id)
        .unwrap()
        .is_empty());
    assert!(matches!(
        store.reserve_review_opening("/repo", "open-2", request("ws-1")),
        Err(StoreError::ReviewWorkspaceBusy { .. })
    ));
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_snapshot")
        .unwrap();
    assert!(store
        .publish_review_opening("/repo", "open-1", opening.version, snapshot(), &[])
        .is_ok());
}

#[test]
fn preparing_a_binding_requires_its_initial_recovery_head() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut opening = store
        .reserve_review_opening("/repo", "open-1", request("ws-1"))
        .unwrap();
    let mut incomplete = binding();
    incomplete.initial_head.clear();
    opening.bindings.push(incomplete);
    assert!(matches!(
        store.save_review_opening(&opening, opening.version),
        Err(StoreError::ReviewPullRequestInvalid(_))
    ));
}

fn received_snapshot() -> (ReviewSnapshot, Vec<ReviewBranchBinding>) {
    let mut next = snapshot();
    next.id = "snapshot-received".into();
    next.author = Actor::Build;
    next.directories[0].head = Some("def456".into());
    let mut next_binding = binding();
    next_binding.last_received_head = Some("def456".into());
    (next, vec![next_binding])
}

fn assert_received_publication(
    store: &Store,
    review: &crate::reviews::records::Review,
    saved: &crate::reviews::records::Review,
    bindings: &[ReviewBranchBinding],
) {
    assert_eq!(saved.version, 2);
    assert_eq!(saved.snapshots.len(), 1);
    assert_eq!(saved.snapshots[0].number, 2);
    let history = store.load_review(&review.task_id).unwrap().unwrap();
    assert_eq!(history.snapshots.len(), 2);
    assert_eq!(history.snapshots[0], review.snapshots[0]);
    assert_eq!(saved.bindings, bindings);
    assert_eq!(
        saved
            .pull_request
            .as_ref()
            .unwrap()
            .latest_published_snapshot_id,
        Some("snapshot-received".into())
    );
}

fn assert_received_move(store: &Store, task_id: &str) {
    assert_eq!(
        store.load_tracker_task(task_id).unwrap().unwrap().status,
        "in_review"
    );
    let timeline = store.load_tracker_timeline(task_id).unwrap();
    let crate::tracker::TimelineEntry::Event(event) = &timeline[0] else {
        panic!("receiving a snapshot must append a move event");
    };
    assert_eq!(event.actor, Actor::Build);
    assert_eq!(event.kind, crate::tracker::TaskEventKind::Moved);
    assert_eq!(
        event.payload,
        serde_json::json!({"from": "in_progress", "to": "in_review"})
    );
}

#[test]
fn received_snapshot_appends_atomically_and_moves_task_with_build_activity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let mut task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    task.status = "in_progress".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let (next, bindings) = received_snapshot();
    store.fail_next_write();
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, next.clone(), &bindings)
        .is_err());
    assert_eq!(store.load_review(&review.task_id).unwrap().unwrap(), review);
    let saved = store
        .save_review_received_snapshot(&review.task_id, 1, next.clone(), &bindings)
        .unwrap();
    assert_received_publication(&store, &review, &saved, &bindings);
    assert_received_move(&store, &review.task_id);
    assert!(matches!(
        store.save_review_received_snapshot(&review.task_id, 1, next.clone(), &bindings),
        Err(StoreError::ReviewVersionConflict { .. })
    ));
    assert!(matches!(
        store.save_review_received_snapshot(&review.task_id, 2, next, &bindings),
        Err(StoreError::ReviewSnapshotExists { .. })
    ));
    assert_eq!(
        store
            .load_review(&review.task_id)
            .unwrap()
            .unwrap()
            .snapshots
            .len(),
        2
    );
}

#[test]
fn received_snapshot_rejects_changed_binding_membership_and_terminal_reviews() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let (next, bindings) = received_snapshot();
    let mut altered = bindings.clone();
    altered[0].repository_id = "other".into();
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, next.clone(), &altered)
        .is_err());
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, next.clone(), &[])
        .is_err());
    let mut duplicate_bindings = bindings.clone();
    duplicate_bindings.push(bindings[0].clone());
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, next.clone(), &duplicate_bindings)
        .is_err());
    let mut missing = next.clone();
    missing.directories.clear();
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, missing, &bindings)
        .is_err());
    let mut changed_source = next.clone();
    changed_source.directories[0].source_id = "other".into();
    assert!(store
        .save_review_received_snapshot(&review.task_id, 1, changed_source, &bindings)
        .is_err());
    assert_eq!(store.load_review(&review.task_id).unwrap().unwrap(), review);
    let closed = store
        .complete_review(&review.task_id, 1, &Actor::User, "closed")
        .unwrap();
    assert!(store
        .save_review_received_snapshot(&review.task_id, closed.version, next, &bindings)
        .is_err());
}

#[test]
fn active_sync_tasks_use_bounded_sorted_database_pages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    assert!(store
        .list_active_review_sync_tasks(None, 0)
        .unwrap()
        .is_empty());
    assert_eq!(
        store.list_active_review_sync_tasks(None, 1).unwrap(),
        vec![review.task_id.clone()]
    );
    assert!(store
        .list_active_review_sync_tasks(Some(&review.task_id), 1)
        .unwrap()
        .is_empty());
    let mut opening = store
        .reserve_review_opening("/repo", "open-page", request("ws-page"))
        .unwrap();
    let mut second_binding = binding();
    second_binding.repository_id = "repo-page".into();
    second_binding.receiving_repository = "/receivers/repo-page.git".into();
    opening.bindings.push(second_binding);
    let opening = store
        .save_review_opening(&opening, opening.version)
        .unwrap();
    let mut second_snapshot = snapshot();
    second_snapshot.id = "snapshot-page".into();
    second_snapshot.directories[0].common_git_dir = Some("/receivers/repo-page.git".into());
    let second = store
        .publish_review_opening("/repo", "open-page", opening.version, second_snapshot, &[])
        .unwrap();
    let mut sorted = vec![review.task_id.clone(), second.task_id.clone()];
    sorted.sort();
    assert_eq!(
        store.list_active_review_sync_tasks(None, 1).unwrap(),
        sorted[..1]
    );
    assert_eq!(
        store
            .list_active_review_sync_tasks(Some(&sorted[0]), 1)
            .unwrap(),
        sorted[1..]
    );
    let snapshot_task = store
        .create_tracker_task(Task::drafted("/repo", "snapshot", Actor::User, NOW), &[])
        .unwrap();
    let mut legacy = snapshot();
    legacy.id = "snapshot-legacy".into();
    store
        .save_review_snapshot(&snapshot_task.id, "ws-legacy", 0, legacy)
        .unwrap();
    assert_eq!(
        store.list_active_review_sync_tasks(None, 100).unwrap(),
        sorted
    );
    store
        .complete_review(&review.task_id, 1, &Actor::User, "closed")
        .unwrap();
    assert_eq!(
        store.list_active_review_sync_tasks(None, 100).unwrap(),
        vec![second.task_id]
    );
}

#[test]
fn received_snapshot_preserves_snapshot_scoped_opinions_and_resets_approval() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let opinion: crate::tracker::TaskComment = serde_json::from_value(serde_json::json!({
        "id": "comment-approval", "task_id": review.task_id, "author": {"kind": "user"},
        "body": "Approved", "created_at": NOW,
        "opinion": {"snapshot_id": "snapshot-1", "verdict": "approve"}
    }))
    .unwrap();
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&opinion), &[])
        .unwrap();
    let approved = store.load_review(&review.task_id).unwrap().unwrap();
    assert_eq!(
        approved.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Approved
    );
    let (next, bindings) = received_snapshot();
    let saved = store
        .save_review_received_snapshot(&review.task_id, approved.version, next, &bindings)
        .unwrap();
    assert_eq!(saved.pull_request.unwrap().status, PullRequestStatus::Open);
    assert_eq!(
        store.load_tracker_timeline(&review.task_id).unwrap(),
        vec![crate::tracker::TimelineEntry::Comment(opinion)]
    );
    assert_eq!(saved.snapshots.len(), 1);
    assert_eq!(saved.snapshots[0].id, "snapshot-received");
    let history = store.load_review(&review.task_id).unwrap().unwrap();
    assert_eq!(history.snapshots[0].id, "snapshot-1");
    assert_eq!(history.snapshots[1].id, "snapshot-received");
}

#[test]
fn sync_state_and_publication_read_only_latest_snapshot_without_decoding_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let (next, bindings) = received_snapshot();
    store
        .save_review_received_snapshot(&review.task_id, 1, next, &bindings)
        .unwrap();
    let conn = store.connection();
    conn.execute(
        "UPDATE review_snapshots SET record = 'invalid old snapshot' WHERE id = 'snapshot-1'",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO review_actions (id, task_id, status, record) VALUES ('old-action', ?1, 'completed', 'invalid old action')", [&review.task_id]).unwrap();
    drop(conn);
    assert!(store.load_review(&review.task_id).is_err());
    let state = store
        .load_review_sync_state(&review.task_id)
        .unwrap()
        .unwrap();
    assert_eq!(state.version, 2);
    assert_eq!(state.snapshots.len(), 1);
    assert_eq!(state.snapshots[0].id, "snapshot-received");
    assert!(state.actions.is_empty());
    assert!(state.destinations.is_empty());
    let (mut newest, mut bindings) = received_snapshot();
    newest.id = "snapshot-newest".into();
    newest.directories[0].head = Some("ghi789".into());
    bindings[0].last_received_head = Some("ghi789".into());
    let saved = store
        .save_review_received_snapshot(&review.task_id, 2, newest, &bindings)
        .unwrap();
    assert_eq!(saved.snapshots.len(), 1);
    assert_eq!(saved.snapshots[0].number, 3);
    assert_eq!(saved.snapshots[0].id, "snapshot-newest");
    assert!(saved.actions.is_empty());
    assert!(store.load_review(&review.task_id).is_err());
    let count: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM review_snapshots WHERE task_id = ?1",
            [&review.task_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "historical rows must remain intact");
}

#[test]
fn sync_state_ignores_legacy_reviews_and_binds_latest_snapshot_to_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    assert!(store.load_review_sync_state("missing").unwrap().is_none());
    let task = store
        .create_tracker_task(Task::drafted("/repo", "legacy", Actor::User, NOW), &[])
        .unwrap();
    let mut legacy = snapshot();
    legacy.id = "snapshot-other-task".into();
    store
        .save_review_snapshot(&task.id, "ws-legacy", 0, legacy)
        .unwrap();
    assert!(store.load_review_sync_state(&task.id).unwrap().is_none());
    store.connection().execute("UPDATE reviews SET record = json_set(record, '$.pull_request.latest_published_snapshot_id', 'snapshot-other-task') WHERE task_id = ?1", [&review.task_id]).unwrap();
    assert!(
        store.load_review_sync_state(&review.task_id).is_err(),
        "another task's snapshot must never be accepted as sync input"
    );
}

#[test]
fn schema_fifteen_reopen_restores_additive_sync_candidate_table_without_rewriting_review() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    store
        .connection()
        .execute_batch("DROP TABLE review_sync_candidates")
        .unwrap();
    drop(store);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(store.load_review(&review.task_id).unwrap().unwrap(), review);
    assert_eq!(
        store.load_review_sync_candidate(&review.task_id).unwrap(),
        None
    );
    assert_eq!(SCHEMA_VERSION, 15);
}

#[test]
fn candidate_registry_identity_is_immutable_and_blocks_history_release_until_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let id = uuid::Uuid::new_v4().to_string();
    store
        .register_review_sync_candidate(&review.task_id, &id)
        .unwrap();
    store
        .register_review_sync_candidate(&review.task_id, &id)
        .unwrap();
    let other = uuid::Uuid::new_v4().to_string();
    assert!(store
        .register_review_sync_candidate(&review.task_id, &other)
        .is_err());
    assert!(store
        .clear_review_sync_candidate(&review.task_id, &other)
        .is_err());
    assert_eq!(
        store.load_review_sync_candidate(&review.task_id).unwrap(),
        Some(id.clone())
    );
    let called = std::cell::Cell::new(false);
    assert!(store
        .delete_tracker_tasks_of_project("/repo", |_| {
            called.set(true);
            Ok(())
        })
        .is_err());
    assert!(
        !called.get(),
        "pending pins must block irreversible release callbacks"
    );
    store
        .clear_review_sync_candidate(&review.task_id, &id)
        .unwrap();
    assert!(!store.review_snapshot_is_published(&id).unwrap());
    assert!(store.review_snapshot_is_published("snapshot-1").unwrap());
}

#[test]
fn sync_task_page_work_is_bounded_with_large_active_inactive_and_candidate_sets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    store
        .connection()
        .execute_batch(
            r#"
        WITH RECURSIVE numbers(n) AS (
            SELECT 0 UNION ALL SELECT n + 1 FROM numbers WHERE n < 13999
        ) INSERT INTO reviews (task_id, workspace_id, version, record)
        SELECT printf('task-%05d', n), 'workspace', 1,
            CASE WHEN n < 2000 THEN '{"mode":"pull_request","pull_request":{"status":"open"}}'
            ELSE '{"mode":"pull_request","pull_request":{"status":"closed"}}' END
        FROM numbers;
        WITH RECURSIVE candidates(n) AS (
            SELECT 2000 UNION ALL SELECT n + 1 FROM candidates WHERE n < 3999
        ) INSERT INTO review_sync_candidates (task_id, snapshot_id)
        SELECT printf('task-%05d', n), printf('candidate-%05d', n) FROM candidates;
    "#,
        )
        .unwrap();
    let page = store
        .list_active_review_sync_tasks(Some("task-03899"), 64)
        .unwrap();
    assert_eq!(page.len(), 64);
    assert_eq!(page[0], "task-03900");
    let steps = store.review_sync_scan_steps();
    assert!(steps < 5000, "one late keyset page must avoid scanning all 14000 reviews and 2000 candidates; VM steps={steps}");
}
