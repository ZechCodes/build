use super::tests::{commit_at, job, multi_fixture};
use super::*;
use crate::reviews::sync::reconcile::tests::Fixture;
use std::cell::Cell;

fn partial_shared() -> (Fixture, MergeJob) {
    let fixture = multi_fixture(true);
    commit_at(fixture.checkout(), "first.txt");
    commit_at(&fixture.request.workspace.directories[1].path, "second.txt");
    fixture.sync();
    let selected = job(&fixture);
    let blocked = Cell::new(false);
    let partial = merge_observed(&fixture.store, &selected, &|| {}, &|checkpoint| {
        if checkpoint == MergeCheckpoint::GitRecorded && !blocked.replace(true) {
            std::fs::write(fixture.source.join("README.md"), "dirty shared source\n").unwrap();
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(successful_count(&partial, "directory-1"), 1);
    assert_eq!(successful_count(&partial, "directory-2"), 0);
    (fixture, selected)
}

fn successful_count(review: &Review, directory_id: &str) -> usize {
    review
        .actions
        .iter()
        .filter(|row| row.directory_id == directory_id)
        .flat_map(|row| &row.steps)
        .filter(|step| step.kind == StepKind::Merge && step.status == StepStatus::Succeeded)
        .count()
}

#[test]
fn ordinary_shared_base_retry_cannot_gain_authority_after_external_target_rollback() {
    let (fixture, selected) = partial_shared();
    crate::git_fixture::git_in(
        &fixture.source,
        &[
            "reset",
            "--hard",
            &selected.request.sources[0].expected_base_head,
        ],
    );
    let retried = merge(&fixture.store, &selected, || {}).unwrap();
    assert_eq!(successful_count(&retried, "directory-2"), 0, "{retried:#?}");
    assert!(retried.pull_request.unwrap().status.is_active());
}

fn opinion(fixture: &Fixture) -> Review {
    let review = fixture.review();
    let comment = serde_json::from_value(serde_json::json!({
        "id":"recovery-opinion","task_id":fixture.task_id(),"author":{"kind":"user"},
        "body":"Confirm the current merge targets","created_at":"2026-10-09T02:00:00Z",
        "opinion":{"snapshot_id":review.snapshots.last().unwrap().id,"verdict":"approve"}
    }))
    .unwrap();
    fixture
        .store
        .record_review_opinion(fixture.task_id(), review.version, &comment)
        .unwrap()
}

#[test]
fn shared_base_refresh_keeps_success_and_sequences_the_remaining_source_from_confirmed_tip() {
    let (fixture, selected) = partial_shared();
    crate::git_fixture::git_in(&fixture.source, &["restore", "README.md"]);
    std::fs::write(
        fixture.source.join("base-next.txt"),
        "additional base work\n",
    )
    .unwrap();
    crate::git_fixture::git_in(&fixture.source, &["add", "base-next.txt"]);
    crate::git_fixture::git_in(
        &fixture.source,
        &["commit", "-m", "base advanced after partial integration"],
    );
    let current = opinion(&fixture);
    let target = git2::Repository::open(&fixture.source)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap()
        .to_string();
    let mut confirmed = selected.request.clone();
    confirmed.expected_version = current.version;
    for source in &mut confirmed.sources {
        source.expected_base_head = target.clone();
    }
    let merged = merge_with_refresh(&fixture.store, &selected, Some(&confirmed), || {}).unwrap();
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged,
        "{merged:#?}"
    );
    assert_eq!(successful_count(&merged, "directory-1"), 1);
    assert_eq!(successful_count(&merged, "directory-2"), 1);
    for file in ["first.txt", "second.txt", "base-next.txt"] {
        assert!(fixture.source.join(file).exists());
    }
}

#[test]
fn same_target_vector_refresh_resumes_a_successful_noop_after_an_opinion_version_bump() {
    let fixture = multi_fixture(false);
    commit_at(&fixture.request.workspace.directories[1].path, "second.txt");
    fixture.sync();
    let second = &fixture.request.workspace.directories[1].source_path;
    std::fs::write(second.join("README.md"), "dirty second target\n").unwrap();
    let selected = job(&fixture);
    let partial = merge(&fixture.store, &selected, || {}).unwrap();
    assert_eq!(successful_count(&partial, "directory-1"), 1);
    let current = opinion(&fixture);
    crate::git_fixture::git_in(second, &["restore", "README.md"]);
    let mut confirmed = selected.request.clone();
    confirmed.expected_version = current.version;
    let merged = merge_with_refresh(&fixture.store, &selected, Some(&confirmed), || {}).unwrap();
    assert_eq!(
        merged.pull_request.as_ref().unwrap().status,
        PullRequestStatus::Merged
    );
    assert_eq!(successful_count(&merged, "directory-1"), 1);
    assert_eq!(successful_count(&merged, "directory-2"), 1);
}
