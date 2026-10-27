use super::*;
use crate::reviews::model::*;
use crate::tracker::{Actor, Task};
use std::collections::BTreeMap;

const NOW: &str = "2026-10-08T00:00:00Z";

// Copied bounded Store-level PR fixtures; no git process or live receiver is used.
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

fn assert_review_card(store: &Store, id: &str) {
    let canonical = store.load_review_summary(id).unwrap().unwrap();
    let card = summary(store, "/repo");
    assert_eq!(
        card["review_summary"],
        serde_json::to_value(canonical).unwrap()
    );
    assert_eq!(
        card["status"],
        store.load_tracker_task(id).unwrap().unwrap().status
    );
    assert_matches_history(store, id);
}

#[test]
fn native_pr_publication_opinion_and_projection_failure_are_atomic() {
    use crate::tracker::{ReviewOpinion, ReviewVerdict};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let review = publish(&store);
    let mut watched = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    watched.watched = true;
    store
        .save_tracker_task_activity(&watched, &[], &[])
        .unwrap();
    assert_review_card(&store, &review.task_id);
    let task = store.load_tracker_task(&review.task_id).unwrap().unwrap();
    let mut opinion = said(&task, "tc-00000000000000000000000990", NOW);
    opinion.author = Actor::User;
    opinion.opinion = Some(Box::new(ReviewOpinion {
        snapshot_id: "snapshot-1".into(),
        verdict: ReviewVerdict::Approve,
    }));
    let before = summary(&store, "/repo");
    let history_before = store.load_tracker_timeline(&task.id).unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_pr_metadata BEFORE UPDATE ON tracker_task_summaries BEGIN SELECT RAISE(ABORT,'PR metadata failed'); END;").unwrap();
    assert!(store
        .record_review_opinion(&task.id, review.version, &opinion)
        .is_err());
    assert_eq!(store.load_review(&task.id).unwrap().unwrap(), review);
    assert_eq!(summary(&store, "/repo"), before);
    assert_eq!(
        store.load_tracker_timeline(&task.id).unwrap().len(),
        history_before.len()
    );
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_pr_metadata")
        .unwrap();
    store
        .record_review_opinion(&task.id, review.version, &opinion)
        .unwrap();
    assert_review_card(&store, &task.id);
    assert_eq!(
        summary(&store, "/repo")["review_summary"]["status"],
        "approved"
    );
}
