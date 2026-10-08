use super::*;
use crate::reviews::model::{PullRequestStatus, ReviewMergeSource, ReviewMergeState};
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::{Actor, DONE_STATUS};

fn job(f: &Fixture) -> MergeJob {
    let review = f.review();
    let snapshot = review.snapshots.last().unwrap();
    MergeJob {
        project_path: f.request.project_path.clone(),
        request_id: "merge-1".into(),
        request: ReviewMergeRequest {
            task_id: f.task_id().into(),
            expected_version: review.version,
            snapshot_id: snapshot.id.clone(),
            actor: Actor::User,
            sources: review.bindings.iter().map(|binding| ReviewMergeSource {
                directory_id: binding.directory_id.clone(),
                repository_id: binding.repository_id.clone(),
                base_branch_ref: binding.base_branch_ref.clone(),
                head: binding.last_received_head.clone().unwrap(),
                expected_base_head: git2::Repository::open(&binding.source_repository).unwrap()
                    .refname_to_id(&binding.base_branch_ref).unwrap().to_string(),
                push: None,
            }).collect(),
        },
        sources: snapshot.directories.iter().map(|directory| ActionSource {
            directory: directory.clone(),
            source_path: directory.source_path.clone(),
            error: None,
        }).collect(),
    }
}

#[test]
fn committed_local_merge_completes_without_browser_and_retains_locked_workspace() {
    let f = Fixture::new();
    let head = f.commit("merge.txt");
    f.push();
    f.sync();
    let mut workspace = load_workspace(&f);
    workspace.locked = true;
    crate::workspace::persist_review_workspace(&workspace).unwrap();
    let job = job(&f);
    let merged = merge(&f.store, &job, || {}).unwrap();
    assert_eq!(merged.pull_request.unwrap().status, PullRequestStatus::Merged);
    assert_eq!(f.store.load_tracker_task(f.task_id()).unwrap().unwrap().status, DONE_STATUS);
    let repo = git2::Repository::open(&f.source).unwrap();
    assert!(repo.graph_descendant_of(repo.refname_to_id("refs/heads/main").unwrap(),
        git2::Oid::from_str(&head).unwrap()).unwrap());
    assert!(f.checkout().exists());
    assert!(load_workspace(&f).locked);
    assert_eq!(f.store.load_review_merge_intent(&job.project_path, &job.request_id).unwrap()
        .unwrap().state, ReviewMergeState::Succeeded);
    let tip = repo.refname_to_id("refs/heads/main").unwrap();
    assert_eq!(merge(&f.store, &job, || {}).unwrap(), f.review());
    assert_eq!(repo.refname_to_id("refs/heads/main").unwrap(), tip);
}

#[test]
fn admission_reconciles_terminal_push_and_rejects_stale_selected_snapshot() {
    let f = Fixture::new();
    let stale = job(&f);
    let head = f.commit("newer.txt");
    f.push();
    let error = merge(&f.store, &stale, || {}).unwrap_err();
    assert!(error.contains("version") || error.contains("stale"), "{error}");
    assert_eq!(f.review().snapshots.last().unwrap().directories[0].head.as_deref(), Some(head.as_str()));
    assert!(f.store.load_review_merge_intent(&stale.project_path, &stale.request_id).unwrap().is_none());
}

fn load_workspace(f: &Fixture) -> crate::workspace::Workspace {
    serde_json::from_slice(&std::fs::read(f.request.workspace.root.join(crate::workspace::MANIFEST_FILE)).unwrap()).unwrap()
}

#[test]
fn merged_base_push_failure_retries_recorded_tip_without_merging_again() {
    let f = Fixture::new();
    f.commit("publish.txt");
    f.push();
    f.sync();
    let external = f._home.path().join("external.git");
    crate::git_fixture::git_in(f._home.path(), &["init", "--bare", external.to_str().unwrap()]);
    crate::git_fixture::git_in(&f.source, &["remote", "add", "external", external.to_str().unwrap()]);
    let hook = external.join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut job = job(&f);
    job.request.sources[0].push = Some(crate::reviews::model::ReviewMergePush {
        remote: "external".into(), branch: "main".into(),
    });
    let merged = merge(&f.store, &job, || {}).unwrap();
    assert_eq!(merged.pull_request.unwrap().status, PullRequestStatus::Merged);
    assert_eq!(f.store.load_review_merge_intent(&job.project_path, &job.request_id).unwrap().unwrap().state,
        ReviewMergeState::Failed);
    let source = git2::Repository::open(&f.source).unwrap();
    let tip = source.refname_to_id("refs/heads/main").unwrap();
    std::fs::remove_file(hook).unwrap();
    let retried = merge(&f.store, &job, || {}).unwrap();
    assert_eq!(source.refname_to_id("refs/heads/main").unwrap(), tip);
    assert_eq!(git2::Repository::open_bare(external).unwrap().refname_to_id("refs/heads/main").unwrap(), tip);
    assert_eq!(retried.actions.iter().flat_map(|action| &action.steps)
        .filter(|step| step.kind == crate::reviews::actions::StepKind::Merge).count(), 1);
}

#[test]
fn stale_target_is_reported_without_integrating_or_completing() {
    let f = Fixture::new();
    f.commit("feature.txt");
    f.push();
    f.sync();
    let job = job(&f);
    std::fs::write(f.source.join("target.txt"), "new target\n").unwrap();
    crate::git_fixture::git_in(&f.source, &["add", "target.txt"]);
    crate::git_fixture::git_in(&f.source, &["commit", "-m", "target"]);
    let before = git2::Repository::open(&f.source).unwrap().refname_to_id("refs/heads/main").unwrap();
    let result = merge(&f.store, &job, || {});
    assert!(result.is_err() || result.unwrap().actions.iter().any(|action|
        action.status == crate::reviews::actions::ActionStatus::Failed));
    assert_eq!(git2::Repository::open(&f.source).unwrap().refname_to_id("refs/heads/main").unwrap(), before);
    assert!(f.review().pull_request.unwrap().status.is_active());
}
