use super::tests::{job, multi_fixture};
use super::*;
use crate::reviews::sync::reconcile::tests::Fixture;

#[test]
fn merge_crash_child() {
    let Some(root) = std::env::var_os("BUILD_PR404_CRASH_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let (project_path, request): (String, ReviewMergeRequest) =
        serde_json::from_slice(&std::fs::read(root.join("merge-job.json")).unwrap()).unwrap();
    let store = Store::new(root.join("db")).unwrap();
    let review = load(&store, &request.task_id).unwrap();
    let job = MergeJob {
        project_path,
        request_id: "merge-1".into(),
        request,
        sources: review
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
    };
    let phase = std::env::var("BUILD_PR404_CRASH_PHASE").unwrap();
    merge_observed(&store, &job, &|| {}, &|checkpoint| {
        if format!("{checkpoint:?}") == phase {
            std::process::exit(47);
        }
        Ok(())
    })
    .unwrap();
    panic!("crash checkpoint was never reached");
}

fn crash(f: &Fixture, phase: &str) {
    let selected = job(f);
    std::fs::write(
        f._home.path().join("merge-job.json"),
        serde_json::to_vec(&(selected.project_path, selected.request)).unwrap(),
    )
    .unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reviews::merge::recovery_tests::merge_crash_child",
            "--nocapture",
        ])
        .env("BUILD_PR404_CRASH_ROOT", f._home.path())
        .env("BUILD_PR404_CRASH_PHASE", phase)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(47),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn restart_finishes_only_a_vector_backed_by_saved_merge_success() {
    let f = Fixture::new();
    f.commit("restart.txt");
    f.push();
    f.sync();
    crash(&f, "BeforeFinalization");
    let tip = git2::Repository::open(&f.source)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    assert!(f.review().pull_request.unwrap().status.is_active());
    assert_eq!(recover(&f.store).unwrap(), 1);
    assert_eq!(
        f.review().pull_request.unwrap().status,
        PullRequestStatus::Merged
    );
    assert_eq!(
        git2::Repository::open(&f.source)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        tip
    );
    assert_eq!(recover(&f.store).unwrap(), 0);
}

#[test]
fn restart_marks_uncertain_git_work_interrupted_without_replaying() {
    let f = Fixture::new();
    f.commit("uncertain.txt");
    f.push();
    f.sync();
    let tip = git2::Repository::open(&f.source)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    crash(&f, "BeforeGit");
    assert_eq!(recover(&f.store).unwrap(), 1);
    assert_eq!(
        git2::Repository::open(&f.source)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        tip
    );
    assert!(f.review().pull_request.unwrap().status.is_active());
    assert_eq!(f.review().actions[0].status, ActionStatus::Interrupted);
    assert!(f.review().actions[0]
        .steps
        .iter()
        .all(|step| step.status == StepStatus::Interrupted));
    assert_eq!(
        f.store
            .load_review_merge_intent(&f.request.project_path, "merge-1")
            .unwrap()
            .unwrap()
            .state,
        ReviewMergeState::Interrupted
    );
}

#[test]
fn restart_marks_admitted_work_interrupted_when_its_receiver_is_unavailable() {
    let f = Fixture::new();
    f.commit("unavailable.txt");
    f.push();
    f.sync();
    crash(&f, "BeforeGit");
    let receiver = f.review().bindings[0].receiving_repository.clone();
    std::fs::rename(&receiver, receiver.with_extension("unavailable")).unwrap();
    assert_eq!(recover(&f.store).unwrap(), 1);
    let review = f.review();
    assert!(review.pull_request.unwrap().status.is_active());
    assert_eq!(review.actions[0].status, ActionStatus::Interrupted);
    assert_eq!(
        f.store
            .load_review_merge_intent(&f.request.project_path, "merge-1")
            .unwrap()
            .unwrap()
            .state,
        ReviewMergeState::Interrupted
    );
    assert_eq!(recover(&f.store).unwrap(), 0);
}

#[test]
fn stale_partial_publication_can_settle_after_a_newer_plan_without_remerging() {
    publication_after_newer_plan(true);
}

#[test]
fn partial_publication_can_settle_after_another_plan_merges_the_same_snapshot() {
    publication_after_newer_plan(false);
}

fn publication_after_newer_plan(new_received_heads: bool) {
    let f = multi_fixture(false);
    super::tests::commit_at(f.checkout(), "first.txt");
    let second = &f.request.workspace.directories[1];
    super::tests::commit_at(&second.path, "second.txt");
    f.sync();
    let external = f._home.path().join("external.git");
    crate::git_fixture::git_in(
        f._home.path(),
        &["init", "--bare", external.to_str().unwrap()],
    );
    crate::git_fixture::git_in(
        &f.source,
        &["remote", "add", "external", external.to_str().unwrap()],
    );
    let hook = external.join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(second.source_path.join("README.md"), "dirty target\n").unwrap();
    let mut old = job(&f);
    old.request.sources[0].push = Some(super::super::model::ReviewMergePush {
        remote: "external".into(),
        branch: "main".into(),
    });
    merge(&f.store, &old, || {}).unwrap();
    assert!(f
        .store
        .workspace_review_publication_pending(&f.review().workspace_id)
        .unwrap());
    if new_received_heads {
        super::tests::commit_at(f.checkout(), "newer-first.txt");
        super::tests::commit_at(&second.path, "newer.txt");
        f.sync();
    }
    std::fs::remove_file(hook).unwrap();
    crate::git_fixture::git_in(&second.source_path, &["restore", "README.md"]);
    let mut newer = job(&f);
    newer.request_id = "merge-2".into();
    newer.request.sources[0].push = old.request.sources[0].push.clone();
    let completed = merge(&f.store, &newer, || {}).unwrap();
    assert_eq!(
        completed.pull_request.unwrap().status,
        PullRequestStatus::Merged
    );
    let before = f
        .review()
        .actions
        .iter()
        .flat_map(|action| &action.steps)
        .filter(|step| step.kind == StepKind::Merge)
        .count();
    let retried = merge(&f.store, &old, || {}).unwrap();
    assert_eq!(
        retried.pull_request.unwrap().status,
        PullRequestStatus::Merged
    );
    assert_eq!(
        f.review()
            .actions
            .iter()
            .flat_map(|action| &action.steps)
            .filter(|step| step.kind == StepKind::Merge)
            .count(),
        before
    );
    assert!(!f
        .store
        .workspace_review_publication_pending(&f.review().workspace_id)
        .unwrap());
}

#[test]
fn closed_partial_plan_can_publish_saved_success_without_merging_missing_sources() {
    let f = multi_fixture(false);
    super::tests::commit_at(f.checkout(), "first.txt");
    let second = &f.request.workspace.directories[1];
    super::tests::commit_at(&second.path, "second.txt");
    f.sync();
    let external = f._home.path().join("external.git");
    crate::git_fixture::git_in(
        f._home.path(),
        &["init", "--bare", external.to_str().unwrap()],
    );
    crate::git_fixture::git_in(
        &f.source,
        &["remote", "add", "external", external.to_str().unwrap()],
    );
    let hook = external.join("hooks/pre-receive");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(second.source_path.join("README.md"), "dirty target\n").unwrap();
    let mut selected = job(&f);
    selected.request.sources[0].push = Some(super::super::model::ReviewMergePush {
        remote: "external".into(),
        branch: "main".into(),
    });
    merge(&f.store, &selected, || {}).unwrap();
    let mut task = f.store.load_tracker_task(f.task_id()).unwrap().unwrap();
    let now = crate::store::now_rfc3339();
    task.state = crate::tracker::TaskState::Closed;
    task.closed_at = Some(now.clone());
    let event = crate::tracker::TaskEvent::new(
        f.task_id(),
        crate::tracker::Actor::User,
        crate::tracker::TaskEventKind::Closed,
        serde_json::json!({}),
        &now,
    );
    f.store
        .save_review_task_activity(
            &task,
            &[],
            &[event],
            &crate::tracker::Actor::User,
            None,
            &now,
        )
        .unwrap();
    assert_eq!(
        f.review().pull_request.unwrap().status,
        PullRequestStatus::Closed
    );
    assert!(f
        .store
        .workspace_review_publication_pending(&f.review().workspace_id)
        .unwrap());
    std::fs::remove_file(hook).unwrap();
    crate::git_fixture::git_in(&second.source_path, &["restore", "README.md"]);
    let before = f
        .review()
        .actions
        .iter()
        .flat_map(|action| &action.steps)
        .filter(|step| step.kind == StepKind::Merge)
        .count();
    let second_tip = git2::Repository::open(&second.source_path)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    let published = merge(&f.store, &selected, || {}).unwrap();
    assert_eq!(
        published.pull_request.unwrap().status,
        PullRequestStatus::Closed
    );
    assert_eq!(
        f.review()
            .actions
            .iter()
            .flat_map(|action| &action.steps)
            .filter(|step| step.kind == StepKind::Merge)
            .count(),
        before
    );
    assert_eq!(
        git2::Repository::open(&second.source_path)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        second_tip
    );
    assert!(!f
        .store
        .workspace_review_publication_pending(&f.review().workspace_id)
        .unwrap());
}
