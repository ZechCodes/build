//! Durable PR work can outlive task completion and client connections.
use super::*;
use crate::reviews::actions::ActionSource;
use crate::reviews::merge::{merge, MergeJob};
use crate::reviews::model::*;
use crate::reviews::opening::{open, OpenReviewRequest};
use crate::reviews::records::Review;
use crate::reviews::sync::reconcile::tests::Hooks;
use crate::store::Store;
use crate::tracker::{Actor, TaskEvent, TaskEventKind, TaskState};
use std::collections::BTreeMap;

struct Fixture {
    home: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    workspace_id: String,
    store: Store,
    review: Review,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let (home, state, _project, workspace_id, original_task) = linked_workspace();
        finish(&state, &original_task);
        let (workspace, store) = {
            let app = state.lock().unwrap();
            (
                app.workspaces.get(&workspace_id).unwrap().clone(),
                app.store.as_ref().unwrap().clone(),
            )
        };
        let directory = &workspace.directories[0];
        let request = OpenReviewRequest {
            project_path: directory.source_path.to_str().unwrap().into(),
            request_id: "open-reclaim-review".into(),
            receiver_root: home.path().join("receivers"),
            request: ReviewOpeningRequest {
                workspace_id: workspace_id.clone(),
                title: "Review reclaim race".into(),
                description: "Committed work".into(),
                creator: Actor::User,
                reviewer: None,
                directories: vec![ReviewMembership {
                    directory_id: directory.id.clone(),
                    source_id: directory.source_id.clone(),
                    kind: ReviewMembershipKind::Git,
                    reason: None,
                }],
                base_branches: BTreeMap::from([(directory.id.clone(), "refs/heads/main".into())]),
            },
            workspace,
        };
        let opened = open(&store, &request, &Hooks).unwrap();
        state.lock().unwrap().workspaces.reload().unwrap();
        let output = build_output_in(&opened.review.bindings[0].working_repository);
        Self {
            home,
            state,
            workspace_id,
            store,
            review: opened.review,
            output,
        }
    }

    fn job(&self, push: bool) -> MergeJob {
        let binding = &self.review.bindings[0];
        MergeJob {
            project_path: binding.source_repository.to_str().unwrap().into(),
            request_id: "merge-reclaim-review".into(),
            request: ReviewMergeRequest {
                task_id: self.review.task_id.clone(),
                expected_version: self.review.version,
                snapshot_id: self.review.snapshots.last().unwrap().id.clone(),
                actor: Actor::User,
                sources: vec![ReviewMergeSource {
                    directory_id: binding.directory_id.clone(),
                    repository_id: binding.repository_id.clone(),
                    base_branch_ref: binding.base_branch_ref.clone(),
                    head: binding.last_received_head.clone().unwrap(),
                    expected_base_head: git2::Repository::open(&binding.source_repository)
                        .unwrap()
                        .refname_to_id(&binding.base_branch_ref)
                        .unwrap()
                        .to_string(),
                    push: push.then(|| ReviewMergePush {
                        remote: "origin".into(),
                        branch: "main".into(),
                    }),
                }],
            },
            sources: self
                .review
                .snapshots
                .last()
                .unwrap()
                .directories
                .iter()
                .map(|directory| ActionSource {
                    directory: directory.clone(),
                    source_path: directory.source_path.clone(),
                    error: None,
                })
                .collect(),
        }
    }

    fn close(&self) {
        self.store
            .complete_review(
                &self.review.task_id,
                self.review.version,
                &Actor::User,
                "Finished review",
            )
            .unwrap();
    }

    fn failed_local_intent(&self) -> ReviewMergeIntent {
        let job = self.job(false);
        let mut intent = self
            .store
            .reserve_review_merge(&job.project_path, &job.request_id, job.request)
            .unwrap();
        intent.state = ReviewMergeState::Failed;
        let saved = self
            .store
            .save_review_merge_intent(&intent, intent.version)
            .unwrap();
        self.close();
        saved
    }

    fn assert_retained(&self, hold: &str) {
        AppState::sweep_workspaces(&self.state, &pruning(), now_ms());
        let verdict = lifecycle(&self.state, &self.workspace_id);
        assert!(holds(&verdict).contains(&hold.to_string()), "{verdict}");
        assert_eq!(verdict["reclaimable"], false, "{verdict}");
        assert_eq!(verdict["pruned_bytes"], 0, "{verdict}");
        assert!(self.output.exists(), "publication retains build output");
        let reply = call(
            &self.state,
            "workspace.reclaim",
            json!({"workspace_id": self.workspace_id}),
        );
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["error_code"], "conflict", "{reply}");
        assert!(self
            .state
            .lock()
            .unwrap()
            .workspaces
            .get(&self.workspace_id)
            .unwrap()
            .root
            .exists());
        assert!(!self
            .state
            .lock()
            .unwrap()
            .workspace_reserved(&self.workspace_id));
    }
}

#[test]
fn admitted_merge_retains_workspace_and_build_output_after_task_completion() {
    let f = Fixture::new();
    let job = f.job(false);
    f.store
        .reserve_review_merge(&job.project_path, &job.request_id, job.request)
        .unwrap();
    f.close();
    f.assert_retained("review_publication_pending");
}

#[test]
fn locally_merged_pr_with_failed_external_publication_remains_held_after_task_closes() {
    let f = Fixture::new();
    let binding = &f.review.bindings[0];
    let missing = f.home.path().join("unavailable-origin.git");
    git_in(
        &binding.source_repository,
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );
    let merged = merge(&f.store, &f.job(true), || {}).unwrap();
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{merged:?}"
    );
    let intent = f
        .store
        .load_review_merge_intent(
            binding.source_repository.to_str().unwrap(),
            "merge-reclaim-review",
        )
        .unwrap()
        .unwrap();
    assert_eq!(intent.state, ReviewMergeState::Failed);
    f.assert_retained("review_publication_pending");
    let mut task = f
        .store
        .load_tracker_task(&f.review.task_id)
        .unwrap()
        .unwrap();
    task.state = TaskState::Closed;
    let now = crate::store::now_rfc3339();
    task.closed_at = Some(now.clone());
    let event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Closed,
        json!({}),
        &now,
    );
    f.store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    assert_eq!(
        f.store
            .load_review(&task.id)
            .unwrap()
            .unwrap()
            .pull_request
            .unwrap()
            .status,
        PullRequestStatus::Merged
    );
    f.assert_retained("review_publication_pending");
}

#[test]
fn unreadable_publication_state_fails_closed_for_pruning_and_explicit_reclaim() {
    let f = Fixture::new();
    f.close();
    let connection = rusqlite::Connection::open(f.home.path().join("store/build.db")).unwrap();
    connection
        .execute_batch("DROP TABLE review_merge_intents")
        .unwrap();
    f.assert_retained("review_publication_unread");
}

#[test]
fn publication_rechecked_after_prune_validation_retains_build_output() {
    let f = Fixture::new();
    let mut intent = f.failed_local_intent();
    intent.state = ReviewMergeState::Running;
    let raced = std::cell::Cell::new(false);
    AppState::sweep_workspaces_racing(
        &f.state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Validated, || {
            f.store
                .save_review_merge_intent(&intent, intent.version)
                .unwrap();
            raced.set(true);
        }),
    );
    assert!(raced.get(), "the prune reached its final locked check");
    assert!(
        f.output.exists(),
        "late merge admission preserves build output"
    );
    assert!(
        holds(&lifecycle(&f.state, &f.workspace_id)).contains(&"review_publication_pending".into())
    );
    assert!(!f.state.lock().unwrap().workspace_reserved(&f.workspace_id));
}

#[test]
fn publication_rechecked_after_explicit_reclaim_measurement_refuses_removal() {
    let f = Fixture::new();
    let mut intent = f.failed_local_intent();
    let params = json!({"workspace_id": f.workspace_id});
    let (answer, deferred) = f
        .state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answer.is_ok(), "{answer:?}");
    let measured = deferred.unwrap().run();
    intent.state = ReviewMergeState::Running;
    f.store
        .save_review_merge_intent(&intent, intent.version)
        .unwrap();
    let decided =
        f.state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, measured);
    let DeferredNext::Answered(refused) = decided else {
        panic!("the removal was handed on after a merge started");
    };
    assert_eq!(
        refused.unwrap_err(),
        "Build cannot reclaim quiet yet: review merge publication is still pending."
    );
    assert!(f.output.exists());
    assert!(!f.state.lock().unwrap().workspace_reserved(&f.workspace_id));
}

#[test]
fn lock_arriving_during_merge_retains_successful_workspace_and_refuses_every_removal() {
    let f = Fixture::new();
    let merged = merge(&f.store, &f.job(false), || {
        let reply = call(
            &f.state,
            "workspace.set_locked",
            json!({"workspace_id": f.workspace_id, "locked": true}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
    })
    .unwrap();
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{merged:?}"
    );
    for method in ["workspace.delete", "workspace.finish", "workspace.reclaim"] {
        let reply = call(&f.state, method, json!({"workspace_id": f.workspace_id}));
        assert_eq!(reply["error_code"], "locked", "{reply}");
    }
    assert!(f.output.exists());
    assert!(
        f.state
            .lock()
            .unwrap()
            .workspaces
            .get(&f.workspace_id)
            .unwrap()
            .locked
    );
}
