use super::*;
use crate::git_fixture::{git_in, init_repo};
use crate::isolation::{record_branch_teardown, BranchTeardown};
use crate::reviews::model::{ReviewMembership, ReviewMembershipKind, ReviewOpeningRequest};
use crate::store::Store;
use crate::tracker::{Actor, Task};
use crate::workspace::{DirectoryStatus, WorkspaceDirectory, WorkspaceStatus};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Hooks {
    dispatches: AtomicUsize,
    fail_at: Option<OpeningStep>,
    panic_at: Option<OpeningStep>,
    fail_dispatch: bool,
    reserved: bool,
}

impl OpeningHooks for Hooks {
    fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
        if self.reserved {
            Err(crate::reclaim::RESERVED.into())
        } else {
            Ok(())
        }
    }

    fn checkpoint(&self, step: OpeningStep) -> Result<(), String> {
        assert_ne!(self.panic_at, Some(step), "simulated process interruption");
        if self.fail_at == Some(step) {
            Err("injected interruption".into())
        } else {
            Ok(())
        }
    }

    fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }

    fn dispatch_reviewer(&self, _: &Task, _: &str) -> Result<(), String> {
        self.dispatches.fetch_add(1, Ordering::SeqCst);
        if self.fail_dispatch {
            return Err("reviewer unavailable".into());
        }
        Ok(())
    }
}

fn hooks() -> Hooks {
    Hooks {
        dispatches: AtomicUsize::new(0),
        fail_at: None,
        panic_at: None,
        fail_dispatch: false,
        reserved: false,
    }
}

fn request(home: &Path, source: &Path) -> OpenReviewRequest {
    let root = home.join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let checkout = root.join("repo");
    git_in(
        source,
        &[
            "worktree",
            "add",
            "-b",
            "build/work",
            checkout.to_str().unwrap(),
        ],
    );
    record_branch_teardown(&checkout, BranchTeardown::DeletesBranch).unwrap();
    let workspace = Workspace {
        id: "workspace-1".into(),
        project_id: "project-1".into(),
        name: "Changes".into(),
        root,
        status: WorkspaceStatus::Ready,
        archived_at: None,
        isolation: Default::default(),
        managed: true,
        created_by_agent: false,
        locked: false,
        directories: vec![WorkspaceDirectory {
            id: "directory-1".into(),
            source_id: "source-1".into(),
            name: "Source".into(),
            path: checkout,
            source_path: source.into(),
            is_git: true,
            branch: Some("build/work".into()),
            base_branch: "main".into(),
            effective_isolation: Some(Default::default()),
            finished_head: None,
            status: DirectoryStatus::Ready,
            error: None,
        }],
    };
    crate::workspace::persist_review_workspace(&workspace).unwrap();
    OpenReviewRequest {
        project_path: source.to_str().unwrap().into(),
        request_id: "open-1".into(),
        receiver_root: home.join("receivers"),
        request: ReviewOpeningRequest {
            workspace_id: workspace.id.clone(),
            title: "Improve review flow".into(),
            description: "Review committed changes".into(),
            creator: Actor::User,
            reviewer: None,
            directories: vec![ReviewMembership {
                directory_id: "directory-1".into(),
                source_id: "source-1".into(),
                kind: ReviewMembershipKind::Git,
                reason: None,
            }],
            base_branches: BTreeMap::from([("directory-1".into(), "refs/heads/main".into())]),
        },
        workspace,
    }
}

#[test]
fn open_preserves_dirty_index_and_files_and_lost_reply_returns_same_task() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let checkout = &request.workspace.directories[0].path;
    std::fs::write(checkout.join("README.md"), "staged\n").unwrap();
    git_in(checkout, &["add", "README.md"]);
    std::fs::write(checkout.join("README.md"), "unstaged\n").unwrap();
    std::fs::write(checkout.join("new.txt"), "untracked\n").unwrap();
    let index_before = std::fs::read(
        crate::isolation::checkout_git_dir(checkout)
            .unwrap()
            .join("index"),
    )
    .unwrap();
    let first = open(&store, &request, &hooks()).unwrap();
    let second = open(&store, &request, &hooks()).unwrap();
    assert_eq!(first.review, second.review);
    assert_eq!(first.task.id, second.task.id);
    assert_eq!(first.review.snapshots.len(), 1);
    assert_eq!(
        first.review.snapshots[0].directories[0].uncommitted_files,
        Some(2)
    );
    assert_eq!(
        std::fs::read(checkout.join("README.md")).unwrap(),
        b"unstaged\n"
    );
    assert_eq!(
        std::fs::read(
            crate::isolation::checkout_git_dir(checkout)
                .unwrap()
                .join("index")
        )
        .unwrap(),
        index_before
    );
    let repo = git2::Repository::open(checkout).unwrap();
    assert!(repo.find_reference("refs/heads/build/work").is_ok());
    assert!(repo
        .head()
        .unwrap()
        .name()
        .unwrap()
        .starts_with("refs/heads/review/1-"));
    assert_eq!(
        crate::isolation::branch_teardown(checkout).unwrap(),
        BranchTeardown::KeepsBranch
    );
}

#[test]
fn simultaneous_open_returns_one_task_and_snapshot() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let start = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            start.wait();
            open(&store, &request, &hooks())
        });
        let second = scope.spawn(|| {
            start.wait();
            open(&store, &request, &hooks())
        });
        [
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ]
    });
    assert_eq!(results[0].task.id, results[1].task.id);
    assert_eq!(results[0].review.snapshots.len(), 1);
    assert_eq!(
        store
            .list_tracker_tasks(&request.project_path, Default::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reclaim_reservation_refuses_before_reserving_a_task_or_creating_refs() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let mut checks = hooks();
    checks.reserved = true;
    assert!(open(&store, &request, &checks)
        .unwrap_err()
        .contains(crate::reclaim::RESERVED));
    assert!(store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .is_none());
}

#[test]
fn every_opening_checkpoint_recovers_after_store_reopen_without_an_orphan_task() {
    for step in [
        OpeningStep::Reserved,
        OpeningStep::Planned,
        OpeningStep::ReceiverCreated,
        OpeningStep::BranchCreated,
        OpeningStep::RemoteConfigured,
        OpeningStep::RefReceived,
        OpeningStep::SnapshotPinned,
        OpeningStep::WorkspaceRecorded,
        OpeningStep::Published,
    ] {
        let (home, source) = init_repo();
        let db = home.path().join("db");
        let store = Store::new(&db).unwrap();
        let request = request(home.path(), &source);
        let mut checks = hooks();
        checks.panic_at = Some(step);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open(
                &store, &request, &checks
            )))
            .is_err(),
            "{step:?}"
        );
        let opening = store
            .load_review_opening(&request.project_path, &request.request_id)
            .unwrap()
            .unwrap();
        let published = step == OpeningStep::Published;
        assert_eq!(
            store.load_tracker_task(&opening.task.id).unwrap().is_some(),
            published,
            "{step:?}"
        );
        drop(store);
        let store = Store::new(&db).unwrap();
        interrupt_unfinished(&store).unwrap();
        let resumed =
            open(&store, &request, &hooks()).unwrap_or_else(|error| panic!("{step:?}: {error}"));
        assert_eq!(resumed.task.id, opening.task.id, "{step:?}");
        assert_eq!(resumed.review.snapshots.len(), 1);
        assert_eq!(
            store
                .list_tracker_tasks(&request.project_path, Default::default())
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn failed_reviewer_dispatch_is_visible_and_retries_without_reopening() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut request = request(home.path(), &source);
    request.request.reviewer = Some(crate::tracker::Assignee::ProjectAgent);
    let mut checks = hooks();
    checks.fail_dispatch = true;
    let saved = open(&store, &request, &checks).unwrap();
    assert!(matches!(
        saved.reviewer_dispatch,
        ReviewerDispatch::Failed { .. }
    ));
    assert_eq!(checks.dispatches.load(Ordering::SeqCst), 1);
    let replay = open(&store, &request, &checks).unwrap();
    assert_eq!(replay.task.id, saved.task.id);
    assert_eq!(checks.dispatches.load(Ordering::SeqCst), 1);
    let checks = hooks();
    let delivered = retry_reviewer_dispatch(&store, &request, &checks).unwrap();
    assert_eq!(delivered.reviewer_dispatch, ReviewerDispatch::Delivered);
    assert_eq!(delivered.task.id, saved.task.id);
    retry_reviewer_dispatch(&store, &request, &checks).unwrap();
    assert_eq!(checks.dispatches.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_restores_original_branch_and_teardown_and_keeps_other_remote_settings() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let checkout = &request.workspace.directories[0].path;
    git_in(
        checkout,
        &["remote", "add", "build-review", "/an/existing/remote"],
    );
    let mut checks = hooks();
    checks.fail_at = Some(OpeningStep::RefReceived);
    assert!(open(&store, &request, &checks).is_err());
    let planned = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    assert_ne!(planned.bindings[0].remote_name, "build-review");
    let cancelled = cancel(&store, &request, &hooks()).unwrap();
    assert_eq!(cancelled.state, ReviewOpeningState::Cancelled);
    let repo = git2::Repository::open(checkout).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/build/work"));
    assert!(repo
        .find_reference(&planned.bindings[0].dedicated_branch_ref)
        .is_err());
    assert_eq!(
        repo.find_remote("build-review").unwrap().url(),
        Some("/an/existing/remote")
    );
    assert_eq!(
        crate::isolation::branch_teardown(checkout).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert!(store
        .load_tracker_task(&cancelled.task.id)
        .unwrap()
        .is_none());
    let mut other = request;
    other.request_id = "open-2".into();
    assert!(open(&store, &other, &hooks()).is_ok());
}

#[test]
fn retry_and_cancel_refuse_a_replaced_workspace_directory() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut request = request(home.path(), &source);
    let original_path = request.workspace.directories[0].path.clone();
    let mut checks = hooks();
    checks.fail_at = Some(OpeningStep::RefReceived);
    assert!(open(&store, &request, &checks).is_err());
    let opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    let replacement = request.workspace.root.join("replacement");
    git_in(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "build/replacement",
            replacement.to_str().unwrap(),
        ],
    );
    request.workspace.directories[0].path = replacement.clone();
    for error in [
        open(&store, &request, &hooks()).unwrap_err(),
        cancel(&store, &request, &hooks()).unwrap_err(),
    ] {
        assert!(error.contains("placement changed"), "{error}");
    }
    let original = git2::Repository::open(&original_path).unwrap();
    assert_eq!(
        original.head().unwrap().name(),
        Some(opening.bindings[0].dedicated_branch_ref.as_str())
    );
    assert!(original
        .find_remote(&opening.bindings[0].remote_name)
        .is_ok());
    let replacement = git2::Repository::open(replacement).unwrap();
    assert_eq!(
        replacement.head().unwrap().name(),
        Some("refs/heads/build/replacement")
    );
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    request.workspace.directories[0].path = original_path;
    assert_eq!(
        open(&store, &request, &hooks()).unwrap().task.id,
        opening.task.id
    );
}

#[test]
fn cancel_retains_an_externally_advanced_branch_and_the_recoverable_claim() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let mut checks = hooks();
    checks.fail_at = Some(OpeningStep::RefReceived);
    assert!(open(&store, &request, &checks).is_err());
    let opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    let checkout = &opening.bindings[0].working_repository;
    std::fs::write(checkout.join("new.txt"), "new commit\n").unwrap();
    git_in(checkout, &["add", "new.txt"]);
    git_in(checkout, &["commit", "-m", "external work"]);
    let head = git2::Repository::open(checkout)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    assert!(cancel(&store, &request, &hooks()).is_err());
    let repo = git2::Repository::open(checkout).unwrap();
    assert_eq!(repo.head().unwrap().target(), Some(head));
    assert!(repo.find_remote(&opening.bindings[0].remote_name).is_ok());
    assert_eq!(
        store
            .load_review_opening(&request.project_path, &request.request_id)
            .unwrap()
            .unwrap()
            .state,
        ReviewOpeningState::Failed
    );
}

#[test]
fn shared_repository_directories_get_distinct_branches_and_remote_aliases() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut request = request(home.path(), &source);
    let checkout = request.workspace.root.join("second");
    git_in(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "build/second",
            checkout.to_str().unwrap(),
        ],
    );
    record_branch_teardown(&checkout, BranchTeardown::DeletesBranch).unwrap();
    let mut directory = request.workspace.directories[0].clone();
    directory.id = "directory-2".into();
    directory.source_id = "source-2".into();
    directory.path = checkout;
    directory.branch = Some("build/second".into());
    request.workspace.directories.push(directory);
    request.request.directories.push(ReviewMembership {
        directory_id: "directory-2".into(),
        source_id: "source-2".into(),
        kind: ReviewMembershipKind::Git,
        reason: None,
    });
    request
        .request
        .base_branches
        .insert("directory-2".into(), "refs/heads/main".into());
    crate::workspace::persist_review_workspace(&request.workspace).unwrap();
    let saved = open(&store, &request, &hooks()).unwrap();
    assert_eq!(saved.review.bindings.len(), 2);
    let first = &saved.review.bindings[0];
    let second = &saved.review.bindings[1];
    assert_ne!(first.dedicated_branch_ref, second.dedicated_branch_ref);
    assert_ne!(first.remote_name, second.remote_name);
    assert_eq!(first.receiving_repository, second.receiving_repository);
}

#[test]
fn separate_clone_publishes_to_the_same_source_receiver_and_survives_workspace_removal() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut request = request(home.path(), &source);
    let linked = request.workspace.directories[0].path.clone();
    git_in(&source, &["worktree", "remove", linked.to_str().unwrap()]);
    git_in(
        &source,
        &[
            "clone",
            "--no-local",
            source.to_str().unwrap(),
            linked.to_str().unwrap(),
        ],
    );
    crate::git_fixture::configure_repo(&linked);
    git_in(&linked, &["switch", "-c", "build/clone"]);
    crate::isolation::write_rift_marker(&linked, &source).unwrap();
    record_branch_teardown(&linked, BranchTeardown::DeletesBranch).unwrap();
    request.workspace.directories[0].branch = Some("build/clone".into());
    request.workspace.directories[0].effective_isolation = Some(crate::isolation::Isolation::Rift);
    crate::workspace::persist_review_workspace(&request.workspace).unwrap();
    let receiver = receivers::plan_receiver(&source, &request.receiver_root).unwrap();
    let saved = open(&store, &request, &hooks()).unwrap();
    assert_eq!(saved.review.bindings[0].receiving_repository, receiver.path);
    assert_ne!(
        saved.review.snapshots[0].directories[0].common_git_dir,
        Some(linked.join(".git"))
    );
    std::fs::remove_dir_all(&request.workspace.root).unwrap();
    let replay = open(&store, &request, &hooks()).unwrap();
    assert_eq!(replay.task.id, saved.task.id);
    let repository = git2::Repository::open_bare(&receiver.path).unwrap();
    let head = git2::Oid::from_str(
        saved.review.snapshots[0].directories[0]
            .head
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert!(repository.find_commit(head).is_ok());
    assert!(!receiver.path.join("objects/info/alternates").exists());
}

#[test]
fn database_refusal_after_setup_leaves_no_published_task_and_retry_uses_the_reservation() {
    struct FailingPublication<'a>(&'a Store);
    impl OpeningHooks for FailingPublication<'_> {
        fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
            Ok(())
        }
        fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
            Ok(())
        }
        fn dispatch_reviewer(&self, _: &Task, _: &str) -> Result<(), String> {
            panic!("dispatch before publish")
        }
        fn checkpoint(&self, step: OpeningStep) -> Result<(), String> {
            if step == OpeningStep::WorkspaceRecorded {
                self.0.fail_next_write();
            }
            Ok(())
        }
    }
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    assert!(open(&store, &request, &FailingPublication(&store)).is_err());
    let opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    assert!(store.load_tracker_task(&opening.task.id).unwrap().is_none());
    assert!(store.load_review(&opening.task.id).unwrap().is_none());
    let saved = open(&store, &request, &hooks()).unwrap();
    assert_eq!(saved.task.id, opening.task.id);
}

#[test]
fn every_unpublished_checkpoint_can_be_cancelled_and_releases_its_claim() {
    for step in [
        OpeningStep::Reserved,
        OpeningStep::Planned,
        OpeningStep::ReceiverCreated,
        OpeningStep::BranchCreated,
        OpeningStep::RemoteConfigured,
        OpeningStep::RefReceived,
        OpeningStep::SnapshotPinned,
        OpeningStep::WorkspaceRecorded,
    ] {
        let (home, source) = init_repo();
        let store = Store::new(home.path().join("db")).unwrap();
        let request = request(home.path(), &source);
        let mut checks = hooks();
        checks.fail_at = Some(step);
        assert!(open(&store, &request, &checks).is_err(), "{step:?}");
        let cancelled =
            cancel(&store, &request, &hooks()).unwrap_or_else(|error| panic!("{step:?}: {error}"));
        assert_eq!(cancelled.state, ReviewOpeningState::Cancelled);
        assert!(store
            .load_tracker_task(&cancelled.task.id)
            .unwrap()
            .is_none());
        assert!(store
            .load_workspace_review_summary(&request.workspace.id)
            .unwrap()
            .is_none());
        store
            .reserve_review_opening(
                &request.project_path,
                "after-cancel",
                request.request.clone(),
            )
            .unwrap();
    }
}

#[test]
fn durable_branch_reservation_adds_a_stable_collision_suffix_before_git_preparation() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let mut checks = hooks();
    checks.fail_at = Some(OpeningStep::Planned);
    assert!(open(&store, &request, &checks).is_err());
    let reserved = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    let binding = &reserved.bindings[0];
    assert!(!binding.receiving_repository.exists());
    assert_eq!(
        store
            .review_branch_owner(&binding.repository_id, &binding.dedicated_branch_ref)
            .unwrap(),
        Some(reserved.task.id)
    );
    let preview = preview_branches(&store, &request, 1, "another-task").unwrap();
    assert_ne!(
        preview[0].dedicated_branch_ref,
        binding.dedicated_branch_ref
    );
    assert!(preview[0]
        .dedicated_branch_ref
        .starts_with(&binding.dedicated_branch_ref));
    assert_eq!(
        preview_branches(&store, &request, 1, "another-task").unwrap(),
        preview
    );
    assert!(!binding.receiving_repository.exists());
}

#[test]
fn explicit_history_deletion_releases_receiver_head_base_and_target_pins() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let saved = open(&store, &request, &hooks()).unwrap();
    let repository =
        git2::Repository::open_bare(&saved.review.bindings[0].receiving_repository).unwrap();
    assert_eq!(
        repository
            .references_glob("refs/build/reviews/*")
            .unwrap()
            .count(),
        3
    );
    super::super::service::delete_project_history(&store, &request.project_path).unwrap();
    assert_eq!(
        repository
            .references_glob("refs/build/reviews/*")
            .unwrap()
            .count(),
        0
    );
    assert!(repository
        .find_reference(&saved.review.bindings[0].receiving_ref)
        .is_ok());
    assert!(store.load_review(&saved.task.id).unwrap().is_none());
}
