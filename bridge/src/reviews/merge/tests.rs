use super::*;
use crate::reviews::model::{PullRequestStatus, ReviewMergeSource, ReviewMergeState};
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::{Actor, DONE_STATUS};

pub(super) fn job(f: &Fixture) -> MergeJob {
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
            sources: review
                .bindings
                .iter()
                .map(|binding| ReviewMergeSource {
                    directory_id: binding.directory_id.clone(),
                    repository_id: binding.repository_id.clone(),
                    base_branch_ref: binding.base_branch_ref.clone(),
                    head: binding.last_received_head.clone().unwrap(),
                    expected_base_head: git2::Repository::open(&binding.source_repository)
                        .unwrap()
                        .refname_to_id(&binding.base_branch_ref)
                        .unwrap()
                        .to_string(),
                    push: None,
                })
                .collect(),
        },
        sources: snapshot
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
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{merged:#?}"
    );
    assert_eq!(
        f.store
            .load_tracker_task(f.task_id())
            .unwrap()
            .unwrap()
            .status,
        DONE_STATUS
    );
    let repo = git2::Repository::open(&f.source).unwrap();
    assert!(repo
        .graph_descendant_of(
            repo.refname_to_id("refs/heads/main").unwrap(),
            git2::Oid::from_str(&head).unwrap()
        )
        .unwrap());
    assert!(f.checkout().exists());
    assert!(load_workspace(&f).locked);
    assert_eq!(
        f.store
            .load_review_merge_intent(&job.project_path, &job.request_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewMergeState::Succeeded
    );
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
    assert!(
        error.contains("version") || error.contains("stale"),
        "{error}"
    );
    assert_eq!(
        f.review().snapshots.last().unwrap().directories[0]
            .head
            .as_deref(),
        Some(head.as_str())
    );
    assert!(f
        .store
        .load_review_merge_intent(&stale.project_path, &stale.request_id)
        .unwrap()
        .is_none());
}

fn load_workspace(f: &Fixture) -> crate::workspace::Workspace {
    serde_json::from_slice(
        &std::fs::read(
            f.request
                .workspace
                .root
                .join(crate::workspace::MANIFEST_FILE),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn merged_base_push_failure_retries_recorded_tip_without_merging_again() {
    let f = Fixture::new();
    f.commit("publish.txt");
    f.push();
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
    let mut job = job(&f);
    job.request.sources[0].push = Some(crate::reviews::model::ReviewMergePush {
        remote: "external".into(),
        branch: "main".into(),
    });
    let merged = merge(&f.store, &job, || {}).unwrap();
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{merged:#?}"
    );
    assert_eq!(
        f.store
            .load_review_merge_intent(&job.project_path, &job.request_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewMergeState::Failed
    );
    let source = git2::Repository::open(&f.source).unwrap();
    let tip = source.refname_to_id("refs/heads/main").unwrap();
    std::fs::remove_file(hook).unwrap();
    let retried = merge(&f.store, &job, || {}).unwrap();
    assert_eq!(source.refname_to_id("refs/heads/main").unwrap(), tip);
    assert_eq!(
        git2::Repository::open_bare(external)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        tip
    );
    assert_eq!(
        retried
            .actions
            .iter()
            .flat_map(|action| &action.steps)
            .filter(|step| step.kind == crate::reviews::actions::StepKind::Merge)
            .count(),
        1
    );
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
    let before = git2::Repository::open(&f.source)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    let result = merge(&f.store, &job, || {});
    assert!(
        result.is_err()
            || result
                .unwrap()
                .actions
                .iter()
                .any(|action| action.status == crate::reviews::actions::ActionStatus::Failed)
    );
    assert_eq!(
        git2::Repository::open(&f.source)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        before
    );
    assert!(f.review().pull_request.unwrap().status.is_active());
}

#[test]
fn real_receive_pack_cannot_advance_ref_between_verification_and_completion() {
    let f = Fixture::new();
    f.commit("included.txt");
    f.push();
    f.sync();
    let selected = job(&f);
    f.commit("later.txt"); // Committed but not received: preserved as later work.
    let attempted = std::cell::Cell::new(false);
    let result = merge_observed(&f.store, &selected, &|| {}, &|phase| {
        if phase == MergeCheckpoint::RefsLocked {
            let output = crate::git_fixture::git_command(f.checkout(), &["push"])
                .output()
                .unwrap();
            assert!(
                !output.status.success(),
                "receive-pack advanced a fenced ref"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("cannot lock ref"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            attempted.set(true);
        }
        Ok(())
    })
    .unwrap();
    assert!(attempted.get());
    assert_eq!(
        result.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{result:#?}"
    );
    let history = f.review().snapshots;
    f.push();
    f.sync();
    assert_eq!(f.review().snapshots, history);
    assert_eq!(
        f.review().pull_request.unwrap().status,
        PullRequestStatus::Merged
    );
}

#[test]
fn receiving_push_winning_before_finalization_leaves_new_work_open_and_keeps_results() {
    let f = Fixture::new();
    f.commit("included.txt");
    f.push();
    f.sync();
    let selected = job(&f);
    let newer = f.commit("newer.txt");
    let result = merge_observed(&f.store, &selected, &|| {}, &|phase| {
        if phase == MergeCheckpoint::BeforeFinalization {
            f.push();
        }
        Ok(())
    })
    .unwrap();
    assert!(result.pull_request.unwrap().status.is_active());
    assert_eq!(
        result.snapshots.last().unwrap().directories[0]
            .head
            .as_deref(),
        Some(newer.as_str())
    );
    assert!(result.actions.iter().any(|action| action
        .steps
        .iter()
        .any(|step| step.kind == crate::reviews::actions::StepKind::Merge
            && step.status == crate::reviews::actions::StepStatus::Succeeded)));
}

pub(super) fn multi_fixture(shared: bool) -> Fixture {
    use crate::reviews::opening::{open, OpenReviewRequest};
    use crate::reviews::sync::reconcile::tests::{request, Hooks};
    let (home, source) = crate::git_fixture::init_repo();
    let second = if shared {
        source.clone()
    } else {
        crate::git_fixture::init_repo_named(home.path(), "second-source")
    };
    let mut request: OpenReviewRequest = request(home.path(), &source);
    let checkout = request.workspace.root.join("second");
    crate::git_fixture::git_in(
        &second,
        &[
            "worktree",
            "add",
            "-b",
            "build/second",
            checkout.to_str().unwrap(),
        ],
    );
    crate::isolation::record_branch_teardown(
        &checkout,
        crate::isolation::BranchTeardown::DeletesBranch,
    )
    .unwrap();
    let mut directory = request.workspace.directories[0].clone();
    directory.id = "directory-2".into();
    directory.source_id = "source-2".into();
    directory.name = "Second".into();
    directory.path = checkout;
    directory.source_path = second;
    directory.branch = Some("build/second".into());
    request.workspace.directories.push(directory);
    let mut member = request.request.directories[0].clone();
    member.directory_id = "directory-2".into();
    member.source_id = "source-2".into();
    request.request.directories.push(member);
    request
        .request
        .base_branches
        .insert("directory-2".into(), "refs/heads/main".into());
    crate::workspace::persist_review_workspace(&request.workspace).unwrap();
    let store = Store::new(home.path().join("db")).unwrap();
    let opened = open(&store, &request, &Hooks).unwrap();
    Fixture {
        _home: home,
        source,
        store,
        request,
        opened,
    }
}

pub(super) fn commit_at(path: &std::path::Path, file: &str) {
    std::fs::write(path.join(file), file).unwrap();
    crate::git_fixture::git_in(path, &["add", file]);
    crate::git_fixture::git_in(path, &["commit", "-m", file]);
    crate::git_fixture::git_in(path, &["push"]);
}

#[test]
fn shared_repository_base_advances_from_its_own_first_merge() {
    let f = multi_fixture(true);
    commit_at(f.checkout(), "first.txt");
    commit_at(&f.request.workspace.directories[1].path, "second.txt");
    f.sync();
    let result = merge(&f.store, &job(&f), || {
        f.sync();
    })
    .unwrap();
    assert_eq!(
        result.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{result:#?}"
    );
    for file in ["first.txt", "second.txt"] {
        assert!(f.source.join(file).exists());
    }
    assert_eq!(
        result
            .actions
            .iter()
            .filter(|action| action.status == crate::reviews::actions::ActionStatus::Succeeded)
            .count(),
        2
    );
}

#[test]
fn partial_repository_merge_retries_only_missing_source_without_replaying_success() {
    let f = multi_fixture(false);
    commit_at(f.checkout(), "first.txt");
    let second = &f.request.workspace.directories[1];
    commit_at(&second.path, "second.txt");
    f.sync();
    std::fs::write(second.source_path.join("README.md"), "dirty target\n").unwrap();
    let selected = job(&f);
    let partial = merge(&f.store, &selected, || {
        f.sync();
    })
    .unwrap();
    assert!(partial.pull_request.unwrap().status.is_active());
    assert_eq!(
        partial
            .actions
            .iter()
            .filter(|action| action.status == crate::reviews::actions::ActionStatus::Succeeded)
            .count(),
        1
    );
    let tip = git2::Repository::open(&f.source)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    crate::git_fixture::git_in(&second.source_path, &["restore", "README.md"]);
    let complete = merge(&f.store, &selected, || {
        f.sync();
    })
    .unwrap();
    assert_eq!(
        complete.pull_request.unwrap().status,
        PullRequestStatus::Merged
    );
    assert_eq!(
        git2::Repository::open(&f.source)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap(),
        tip
    );
    assert_eq!(
        complete
            .actions
            .iter()
            .filter(|action| action.directory_id == "directory-1")
            .count(),
        1
    );
}
