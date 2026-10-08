use super::reconcile::{
    reconcile,
    tests::{request, Hooks},
};
use crate::git_fixture::{git_in, init_repo, init_repo_named};
use crate::isolation::{record_branch_teardown, BranchTeardown};
use crate::reviews::model::{ReviewMembershipKind, ReviewSyncHealth};
use crate::reviews::opening::open;
use crate::reviews::records::Review;
use crate::store::Store;
use std::path::Path;

fn commit(checkout: &Path, name: &str) -> String {
    std::fs::write(checkout.join(name), name).unwrap();
    git_in(checkout, &["add", name]);
    git_in(checkout, &["commit", "-m", name]);
    git2::Repository::open(checkout)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string()
}

fn heads(review: &Review) -> Vec<(String, Option<String>)> {
    review
        .snapshots
        .last()
        .unwrap()
        .directories
        .iter()
        .map(|directory| (directory.id.clone(), directory.head.clone()))
        .collect()
}

#[test]
fn partial_pushes_publish_complete_vectors_and_missing_participant_recovers_latest_received_set() {
    let (home, source) = init_repo();
    let second_source = init_repo_named(home.path(), "second-source");
    let mut request = request(home.path(), &source);
    let second_checkout = request.workspace.root.join("second");
    git_in(
        &second_source,
        &[
            "worktree",
            "add",
            "-b",
            "build/second",
            second_checkout.to_str().unwrap(),
        ],
    );
    record_branch_teardown(&second_checkout, BranchTeardown::DeletesBranch).unwrap();
    let mut directory = request.workspace.directories[0].clone();
    directory.id = "directory-2".into();
    directory.source_id = "source-2".into();
    directory.name = "Second".into();
    directory.path = second_checkout.clone();
    directory.source_path = second_source;
    directory.branch = Some("build/second".into());
    request.workspace.directories.push(directory);
    let mut membership = request.request.directories[0].clone();
    membership.directory_id = "directory-2".into();
    membership.source_id = "source-2".into();
    assert_eq!(membership.kind, ReviewMembershipKind::Git);
    request.request.directories.push(membership);
    request
        .request
        .base_branches
        .insert("directory-2".into(), "refs/heads/main".into());
    crate::workspace::persist_review_workspace(&request.workspace).unwrap();
    let store = Store::new(home.path().join("db")).unwrap();
    let opened = open(&store, &request, &Hooks).unwrap();
    let task_id = &opened.task.id;
    let first_checkout = &request.workspace.directories[0].path;
    let baseline = heads(&opened.review);
    let first = commit(first_checkout, "first-published.txt");
    git_in(first_checkout, &["push"]);
    assert!(reconcile(&store, task_id).unwrap().persisted);
    let first_published = store.load_review(task_id).unwrap().unwrap();
    assert_eq!(first_published.snapshots.len(), 2);
    assert_eq!(
        heads(&first_published),
        vec![
            ("directory-1".into(), Some(first.clone())),
            baseline[1].clone()
        ]
    );

    // A second participant publishes independently. The first participant's
    // latest receiver state remains part of the newly captured full vector.
    let second = commit(&second_checkout, "second-published.txt");
    git_in(&second_checkout, &["push"]);
    reconcile(&store, task_id).unwrap();
    let both_published = store.load_review(task_id).unwrap().unwrap();
    assert_eq!(both_published.snapshots.len(), 3);
    assert_eq!(
        heads(&both_published),
        vec![
            ("directory-1".into(), Some(first)),
            ("directory-2".into(), Some(second))
        ]
    );
    let second_binding = &both_published.bindings[1];
    git_in(
        &second_checkout,
        &[
            "push",
            "build-review",
            &format!(":{}", second_binding.receiving_ref),
        ],
    );
    let newest_first = commit(first_checkout, "later-first.txt");
    git_in(first_checkout, &["push"]);
    assert!(reconcile(&store, task_id).unwrap().retry);
    let unavailable = store.load_review(task_id).unwrap().unwrap();
    assert_eq!(
        unavailable, both_published,
        "missing required participant must retain prior complete snapshot"
    );
    let observations = store.load_review_sync_observations(task_id).unwrap();
    assert_eq!(observations[1].health, ReviewSyncHealth::Unavailable);
    assert_eq!(
        observations[0].received_head.as_deref(),
        Some(newest_first.as_str())
    );

    // Recovery receives the second participant's newest commit, then captures
    // the actual current pair without rolling either receiver backward.
    let newest_second = commit(&second_checkout, "later-second.txt");
    git_in(&second_checkout, &["push"]);
    assert!(reconcile(&store, task_id).unwrap().persisted);
    let recovered = store.load_review(task_id).unwrap().unwrap();
    assert_eq!(recovered.snapshots.len(), 4);
    assert_eq!(
        heads(&recovered),
        vec![
            ("directory-1".into(), Some(newest_first)),
            ("directory-2".into(), Some(newest_second))
        ]
    );
    assert_eq!(
        &recovered.snapshots[..3],
        both_published.snapshots.as_slice()
    );
    assert!(store
        .load_review_sync_observations(task_id)
        .unwrap()
        .iter()
        .all(|observation| observation.health == ReviewSyncHealth::Current));
}
