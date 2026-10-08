use super::*;
use crate::reviews::model::PullRequestStatus;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::{TaskEvent, TaskEventKind, TaskState};
use crate::workspace::{Workspace, WorkspaceSource};

struct Hooks;
impl ReopenHooks for Hooks {
    fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }
}

fn request(f: &Fixture) -> ReopenRequest {
    ReopenRequest {
        task_id: f.task_id().into(),
        expected_version: f.review().version,
        actor: Actor::User,
        workspace: f.request.workspace.clone(),
        sources: f
            .request
            .workspace
            .directories
            .iter()
            .map(|directory| WorkspaceSource {
                id: directory.source_id.clone(),
                name: directory.name.clone(),
                mount: "repo".into(),
                path: directory.source_path.clone(),
                is_git: true,
                base_branch: "main".into(),
            })
            .collect(),
    }
}

fn close(f: &Fixture) {
    f.store
        .complete_review(
            f.task_id(),
            f.review().version,
            &Actor::User,
            "Closed for now",
        )
        .unwrap();
}

#[test]
fn reopen_always_publishes_a_fresh_snapshot_and_retains_lock() {
    let f = Fixture::new();
    close(&f);
    let mut task = f.store.load_tracker_task(f.task_id()).unwrap().unwrap();
    task.state = TaskState::Closed;
    task.closed_at = Some(crate::store::now_rfc3339());
    let event = TaskEvent::new(
        f.task_id(),
        Actor::User,
        TaskEventKind::Closed,
        serde_json::json!({}),
        &task.updated_at,
    );
    f.store
        .save_tracker_task_activity(&task, &[], &[event])
        .unwrap();
    let mut request = request(&f);
    request.workspace.locked = true;
    crate::workspace::persist_review_workspace(&request.workspace).unwrap();
    let closed = f.review();
    let reopened = reopen(&f.store, &request, &Hooks).unwrap();
    assert_eq!(
        reopened.pull_request.unwrap().status,
        PullRequestStatus::Open
    );
    assert_eq!(reopened.snapshots.len(), closed.snapshots.len() + 1);
    assert_ne!(
        reopened.snapshots.last().unwrap().id,
        closed.snapshots.last().unwrap().id
    );
    assert!(reopened.completion.is_none());
    let task = f.store.load_tracker_task(f.task_id()).unwrap().unwrap();
    assert_eq!(task.state, TaskState::Open);
    assert_eq!(task.status, IN_REVIEW_STATUS);
    assert!(task.closed_at.is_none());
    assert!(task.done_at.is_none());
    let workspace: Workspace = serde_json::from_slice(
        &std::fs::read(request.workspace.root.join(crate::workspace::MANIFEST_FILE)).unwrap(),
    )
    .unwrap();
    assert!(workspace.locked);
}

#[test]
fn reopen_captures_pushes_received_while_closed() {
    let f = Fixture::new();
    close(&f);
    let head = f.commit("after-close.txt");
    f.push();
    f.sync();
    let reopened = reopen(&f.store, &request(&f), &Hooks).unwrap();
    assert_eq!(
        reopened.snapshots.last().unwrap().directories[0]
            .head
            .as_deref(),
        Some(head.as_str())
    );
}

#[test]
fn reopen_missing_workspace_or_source_fails_without_changing_history() {
    let f = Fixture::new();
    close(&f);
    let before = f.review();
    let mut missing_source = request(&f);
    missing_source.sources.clear();
    assert!(reopen(&f.store, &missing_source, &Hooks).is_err());
    assert_eq!(f.review(), before);
    std::fs::remove_file(
        f.request
            .workspace
            .root
            .join(crate::workspace::MANIFEST_FILE),
    )
    .unwrap();
    assert!(reopen(&f.store, &request(&f), &Hooks).is_err());
    assert_eq!(f.review(), before);
}

#[test]
fn reopen_refuses_stale_version_and_competing_workspace_claim() {
    let f = Fixture::new();
    close(&f);
    let before = f.review();
    let mut stale = request(&f);
    stale.expected_version -= 1;
    assert!(reopen(&f.store, &stale, &Hooks).is_err());
    assert_eq!(f.review(), before);
    f.store
        .reserve_review_opening(
            &f.request.project_path,
            "other-review",
            f.request.request.clone(),
        )
        .unwrap();
    assert!(reopen(&f.store, &request(&f), &Hooks).is_err());
    assert_eq!(f.review(), before);
    assert!(f
        .store
        .load_review_sync_candidate(f.task_id())
        .unwrap()
        .is_none());
}

#[test]
fn running_merge_defers_its_own_base_change_but_still_publishes_new_received_heads() {
    use crate::reviews::model::{ReviewMergeRequest, ReviewMergeSource};
    let f = Fixture::new();
    let head = f.commit("merge.txt");
    f.push();
    f.sync();
    let review = f.review();
    let binding = &review.bindings[0];
    let source = git2::Repository::open(&f.source).unwrap();
    let merge_request = ReviewMergeRequest {
        task_id: f.task_id().into(),
        expected_version: review.version,
        snapshot_id: review.snapshots.last().unwrap().id.clone(),
        actor: Actor::User,
        sources: vec![ReviewMergeSource {
            directory_id: binding.directory_id.clone(),
            repository_id: binding.repository_id.clone(),
            base_branch_ref: binding.base_branch_ref.clone(),
            head: head.clone(),
            expected_base_head: source
                .refname_to_id(&binding.base_branch_ref)
                .unwrap()
                .to_string(),
            push: None,
        }],
    };
    f.store
        .reserve_review_merge(&f.request.project_path, "running-merge", merge_request)
        .unwrap();
    crate::git_fixture::git_in(
        &f.source,
        &["merge", "--no-ff", "-m", "Integrate review", &head],
    );
    f.sync();
    assert_eq!(
        f.review().version,
        review.version,
        "own base movement must not invalidate merge finalization"
    );
    let newer = f.commit("new-during-merge.txt");
    f.push();
    f.sync();
    assert!(f.review().version > review.version);
    assert_eq!(
        f.review().snapshots.last().unwrap().directories[0]
            .head
            .as_deref(),
        Some(newer.as_str())
    );
}

#[test]
fn reopen_refuses_changed_branch_push_routing_before_capture() {
    for changed in ["push_refspec", "push_remote", "effective_url"] {
        let f = Fixture::new();
        close(&f);
        let before = f.review();
        let binding = &before.bindings[0];
        let working = git2::Repository::open(f.checkout()).unwrap();
        let mut config = working.config().unwrap();
        match changed {
            "push_refspec" => config
                .set_str(
                    &format!("remote.{}.push", binding.remote_name),
                    "refs/heads/other:refs/heads/wrong",
                )
                .unwrap(),
            "push_remote" => config
                .set_str(
                    &format!(
                        "branch.{}.pushremote",
                        binding
                            .dedicated_branch_ref
                            .strip_prefix("refs/heads/")
                            .unwrap()
                    ),
                    "different-remote",
                )
                .unwrap(),
            _ => config
                .set_str(
                    "url./not-the-owned-receiver.insteadOf",
                    binding.receiving_repository.to_str().unwrap(),
                )
                .unwrap(),
        }
        assert!(
            reopen(&f.store, &request(&f), &Hooks).is_err(),
            "routing tamper: {changed}"
        );
        assert_eq!(f.review(), before);
        assert!(f
            .store
            .load_review_sync_candidate(f.task_id())
            .unwrap()
            .is_none());
    }
}

#[test]
fn terminal_reconcile_observes_later_pushes_and_unavailable_receivers_without_reopening() {
    use crate::reviews::model::ReviewSyncHealth;
    let f = Fixture::new();
    close(&f);
    let before = f.review();
    let head = f.commit("later-received.txt");
    f.push();
    let result = crate::reviews::sync::reconcile::reconcile(&f.store, f.task_id()).unwrap();
    assert!(result.persisted);
    assert_eq!(f.review(), before);
    let observations = f.store.load_review_sync_observations(f.task_id()).unwrap();
    assert_eq!(
        observations[0].received_head.as_deref(),
        Some(head.as_str())
    );
    assert_ne!(observations[0].received_head, observations[0].snapshot_head);
    assert_eq!(observations[0].health, ReviewSyncHealth::Pending);
    let binding = &before.bindings[0];
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    receiver
        .find_reference(&binding.receiving_ref)
        .unwrap()
        .delete()
        .unwrap();
    let result = crate::reviews::sync::reconcile::reconcile(&f.store, f.task_id()).unwrap();
    assert!(result.persisted);
    assert_eq!(f.review(), before);
    let observations = f.store.load_review_sync_observations(f.task_id()).unwrap();
    assert_eq!(observations[0].health, ReviewSyncHealth::Unavailable);
    assert!(observations[0].error.is_some());
}
