use super::*;
use crate::git_fixture::git_in;
use crate::reviews::model::ReviewPublicationReason;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::Actor;

#[path = "tests/tracking_recovery.rs"]
mod tracking_recovery;

fn push_request(fixture: &Fixture, head: String) -> PushRequest {
    let review = fixture.review();
    PushRequest {
        task_id: fixture.task_id().into(),
        expected_version: review.version,
        actor: Actor::User,
        sources: vec![PushSource {
            directory_id: review.bindings[0].directory_id.clone(),
            expected_head: head,
            expected_received_head: review.bindings[0].last_received_head.clone(),
            force_with_lease: false,
        }],
    }
}

#[test]
fn explicit_push_publishes_only_bound_head_and_attributes_snapshot() {
    let fixture = Fixture::new();
    let head = fixture.commit("publish.txt");
    let result = push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    let review = fixture.review();
    assert_eq!(review.snapshots.len(), 2);
    assert_eq!(review.snapshots[1].author, Actor::User);
    assert_eq!(review.snapshots[1].directories[0].head, Some(head));
    assert_eq!(
        push(
            &fixture.store,
            &push_request(
                &fixture,
                review.bindings[0].last_received_head.clone().unwrap()
            )
        )
        .unwrap()
        .sources[0]
            .status,
        PushStatus::Unchanged
    );
}

#[test]
fn explicit_push_advances_owned_upstream_for_terminal_force_with_lease() {
    let fixture = Fixture::new();
    let head = fixture.commit("published.txt");
    push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    let binding = &fixture.review().bindings[0];
    let branch = binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .unwrap();
    let upstream = format!("refs/remotes/{}/{branch}", binding.remote_name);
    let repository = git2::Repository::open(fixture.checkout()).unwrap();
    assert_eq!(
        repository.refname_to_id(&upstream).unwrap().to_string(),
        head
    );
    git_in(
        fixture.checkout(),
        &["commit", "--amend", "-m", "terminal rewrite"],
    );
    git_in(fixture.checkout(), &["push", "--force-with-lease"]);
    assert_eq!(
        crate::reviews::publication::registered_received_head(binding).unwrap(),
        Some(repository.head().unwrap().target().unwrap().to_string())
    );
}

#[test]
fn stale_version_head_and_receiver_lease_preserve_received_ref() {
    let fixture = Fixture::new();
    let head = fixture.commit("publish.txt");
    let mut request = push_request(&fixture, head.clone());
    request.expected_version += 1;
    assert!(push(&fixture.store, &request)
        .unwrap_err()
        .contains("stale"));
    request.expected_version -= 1;
    request.sources[0].expected_head = fixture.review().bindings[0].initial_head.clone();
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Failed
    );
    request.sources[0].expected_head = head;
    request.sources[0].expected_received_head = None;
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Failed
    );
    assert_eq!(fixture.review().snapshots.len(), 1);
}

#[test]
fn explicit_rewrite_requires_force_and_exact_receiver_lease() {
    let fixture = Fixture::new();
    let old = fixture.commit("old.txt");
    push(&fixture.store, &push_request(&fixture, old)).unwrap();
    git_in(
        fixture.checkout(),
        &["commit", "--amend", "-m", "rewritten"],
    );
    let new = git2::Repository::open(fixture.checkout())
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    let mut request = push_request(&fixture, new);
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Failed
    );
    request.sources[0].force_with_lease = true;
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Published
    );
    assert!(
        fixture.review().snapshots[2]
            .publication
            .as_ref()
            .unwrap()
            .directories[0]
            .rewritten
    );
}

#[test]
fn retarget_always_publishes_new_context_and_retains_history() {
    let fixture = Fixture::new();
    let head = fixture.commit("review.txt");
    push(&fixture.store, &push_request(&fixture, head)).unwrap();
    let before = fixture.review();
    git_in(&fixture.source, &["branch", "alternate", "main"]);
    let request = BaseRequest {
        task_id: fixture.task_id().into(),
        expected_version: before.version,
        actor: Actor::User,
        bases: vec![BaseSelection {
            directory_id: before.bindings[0].directory_id.clone(),
            branch: "alternate".into(),
        }],
    };
    update_bases(&fixture.store, &request).unwrap();
    let after = fixture.review();
    assert_eq!(&after.snapshots[..2], before.snapshots.as_slice());
    assert_eq!(after.snapshots.len(), 3);
    assert_eq!(after.snapshots[2].author, Actor::User);
    assert_eq!(
        after.snapshots[2].publication.as_ref().unwrap().reason,
        ReviewPublicationReason::BaseChanged
    );
    assert_eq!(after.bindings[0].base_branch_ref, "refs/heads/alternate");
    assert!(update_bases(&fixture.store, &request)
        .unwrap_err()
        .contains("stale"));
}

#[test]
fn invalid_retarget_leaves_bindings_snapshots_and_candidates_unchanged() {
    let fixture = Fixture::new();
    let before = fixture.review();
    for branch in ["HEAD", "refs/tags/main", "missing", "../main"] {
        let request = BaseRequest {
            task_id: fixture.task_id().into(),
            expected_version: before.version,
            actor: Actor::User,
            bases: vec![BaseSelection {
                directory_id: before.bindings[0].directory_id.clone(),
                branch: branch.into(),
            }],
        };
        assert!(update_bases(&fixture.store, &request).is_err(), "{branch}");
        assert_eq!(fixture.review(), before);
        assert!(fixture
            .store
            .load_review_sync_candidate(fixture.task_id())
            .unwrap()
            .is_none());
    }
}

#[test]
fn moved_bound_head_during_import_does_not_publish_the_requested_stale_commit() {
    let fixture = Fixture::new();
    let head = fixture.commit("requested.txt");
    let request = push_request(&fixture, head);
    let checks = std::cell::Cell::new(0);
    let result = push_checked(&fixture.store, &request, &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            fixture.commit("advanced.txt");
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    assert!(result.sources[0]
        .error
        .as_ref()
        .unwrap()
        .contains("changed"));
    assert_eq!(fixture.review().snapshots.len(), 1);
}

#[test]
fn review_version_change_under_receiver_locks_fences_final_publication() {
    let fixture = Fixture::new();
    let head = fixture.commit("requested.txt");
    let request = push_request(&fixture, head);
    let before = fixture.review();
    let checks = std::cell::Cell::new(0);
    let result = push_checked(&fixture.store, &request, &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 3 {
            fixture
                .store
                .complete_review(
                    fixture.task_id(),
                    before.version,
                    &Actor::User,
                    "Close while importing",
                )
                .unwrap();
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Failed);
    assert!(result.sources[0]
        .error
        .as_ref()
        .unwrap()
        .starts_with("stale_version:"));
    let binding = &before.bindings[0];
    assert_eq!(
        crate::reviews::publication::registered_received_head(binding).unwrap(),
        binding.last_received_head
    );
    assert_eq!(fixture.review().snapshots, before.snapshots);
}

#[test]
fn retarget_rejects_target_movement_and_cleans_only_its_candidate() {
    let fixture = Fixture::new();
    let head = fixture.commit("requested.txt");
    push(&fixture.store, &push_request(&fixture, head.clone())).unwrap();
    git_in(&fixture.source, &["branch", "alternate", "main"]);
    let before = fixture.review();
    let request = BaseRequest {
        task_id: fixture.task_id().into(),
        expected_version: before.version,
        actor: Actor::User,
        bases: vec![BaseSelection {
            directory_id: before.bindings[0].directory_id.clone(),
            branch: "alternate".into(),
        }],
    };
    let checks = std::cell::Cell::new(0);
    let result = update_bases_checked(&fixture.store, &request, &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            git_in(
                &fixture.source,
                &["update-ref", "refs/heads/alternate", &head],
            );
        }
        Ok(())
    });
    assert!(result.unwrap_err().starts_with("stale:"));
    assert_eq!(fixture.review(), before);
    assert!(fixture
        .store
        .load_review_sync_candidate(fixture.task_id())
        .unwrap()
        .is_none());
}

#[test]
fn retarget_stale_final_transaction_retains_old_binding_and_history() {
    let fixture = Fixture::new();
    git_in(&fixture.source, &["branch", "alternate", "main"]);
    let before = fixture.review();
    let request = BaseRequest {
        task_id: fixture.task_id().into(),
        expected_version: before.version,
        actor: Actor::User,
        bases: vec![BaseSelection {
            directory_id: before.bindings[0].directory_id.clone(),
            branch: "alternate".into(),
        }],
    };
    let checks = std::cell::Cell::new(0);
    let result = update_bases_checked(&fixture.store, &request, &|| {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            fixture
                .store
                .complete_review(
                    fixture.task_id(),
                    before.version,
                    &Actor::User,
                    "Close during retarget",
                )
                .unwrap();
        }
        Ok(())
    });
    assert!(result.unwrap_err().starts_with("stale_version:"));
    let after = fixture.review();
    assert_eq!(after.bindings, before.bindings);
    assert_eq!(after.snapshots, before.snapshots);
    assert!(fixture
        .store
        .load_review_sync_candidate(fixture.task_id())
        .unwrap()
        .is_none());
}

#[test]
fn receiver_user_lock_and_changed_remote_routing_preserve_all_received_refs() {
    let fixture = Fixture::new();
    let head = fixture.commit("requested.txt");
    let request = push_request(&fixture, head);
    let before = fixture.review();
    let binding = &before.bindings[0];
    let lock = binding
        .receiving_repository
        .join(format!("{}.lock", binding.receiving_ref));
    std::fs::write(&lock, b"user lock").unwrap();
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Failed
    );
    assert_eq!(std::fs::read(&lock).unwrap(), b"user lock");
    std::fs::remove_file(lock).unwrap();
    git_in(
        fixture.checkout(),
        &[
            "config",
            &format!("remote.{}.pushurl", binding.remote_name),
            "https://example.test/foreign.git",
        ],
    );
    assert_eq!(
        push(&fixture.store, &request).unwrap().sources[0].status,
        PushStatus::Failed
    );
    assert_eq!(fixture.review(), before);
}

#[test]
fn partial_push_returns_each_result_and_publishes_complete_received_vector() {
    use crate::git_fixture::{init_repo, init_repo_named};
    use crate::isolation::{record_branch_teardown, BranchTeardown};
    use crate::reviews::opening::open;
    use crate::reviews::sync::reconcile::tests::{request, Hooks};
    let (home, source) = init_repo();
    let second_source = init_repo_named(home.path(), "second-source");
    let mut opening = request(home.path(), &source);
    let second = opening.workspace.root.join("second");
    git_in(
        &second_source,
        &[
            "worktree",
            "add",
            "-b",
            "build/second",
            second.to_str().unwrap(),
        ],
    );
    record_branch_teardown(&second, BranchTeardown::DeletesBranch).unwrap();
    let mut directory = opening.workspace.directories[0].clone();
    directory.id = "directory-2".into();
    directory.source_id = "source-2".into();
    directory.path = second.clone();
    directory.source_path = second_source;
    directory.branch = Some("build/second".into());
    opening.workspace.directories.push(directory);
    let mut member = opening.request.directories[0].clone();
    member.directory_id = "directory-2".into();
    member.source_id = "source-2".into();
    opening.request.directories.push(member);
    opening
        .request
        .base_branches
        .insert("directory-2".into(), "refs/heads/main".into());
    crate::workspace::persist_review_workspace(&opening.workspace).unwrap();
    let store = Store::new(home.path().join("db")).unwrap();
    let opened = open(&store, &opening, &Hooks).unwrap();
    let first = &opening.workspace.directories[0].path;
    git_in(first, &["commit", "--allow-empty", "-m", "publish first"]);
    let first_head = git2::Repository::open(first)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    git_in(
        &second,
        &["commit", "--allow-empty", "-m", "new second head"],
    );
    let result = push(
        &store,
        &PushRequest {
            task_id: opened.task.id.clone(),
            expected_version: opened.review.version,
            actor: Actor::User,
            sources: vec![
                PushSource {
                    directory_id: "directory-1".into(),
                    expected_head: first_head.clone(),
                    expected_received_head: opened.review.bindings[0].last_received_head.clone(),
                    force_with_lease: false,
                },
                PushSource {
                    directory_id: "directory-2".into(),
                    expected_head: opened.review.bindings[1].initial_head.clone(),
                    expected_received_head: opened.review.bindings[1].last_received_head.clone(),
                    force_with_lease: false,
                },
            ],
        },
    )
    .unwrap();
    assert_eq!(result.sources[0].status, PushStatus::Published);
    assert_eq!(result.sources[1].status, PushStatus::Failed);
    assert_eq!(result.review.snapshots.len(), 2);
    let latest = &result.review.snapshots[1];
    assert_eq!(
        latest.directories[0].head.as_deref(),
        Some(first_head.as_str())
    );
    assert_eq!(
        latest.directories[1].head,
        opened.review.bindings[1].last_received_head
    );
    assert_eq!(latest.author, Actor::User);
}
