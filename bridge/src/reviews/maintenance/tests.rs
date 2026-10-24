use super::*;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::Actor;

#[test]
fn closed_receiver_is_retained_until_original_workspace_is_removed() {
    let f = Fixture::new();
    let review = f.review();
    f.store
        .complete_review(f.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    let receiver = review.bindings[0].receiving_repository.clone();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(receiver.is_dir(), "original workspace allows reopening");
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    worker.turn().unwrap();
    assert!(!receiver.exists(), "abandoned review releases the receiver");
    worker.turn().unwrap();
}

#[test]
fn open_review_and_pending_capture_prevent_deletion() {
    let f = Fixture::new();
    let receiver = f.review().bindings[0].receiving_repository.clone();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    worker.turn().unwrap();
    assert!(receiver.is_dir(), "open review outlives its workspace");
}

#[test]
fn live_sync_lease_fences_terminal_receiver_cleanup() {
    let f = Fixture::new();
    let review = f.review();
    f.store
        .complete_review(f.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    let held = crate::reviews::sync::reconcile::recovery::CaptureJournal::acquire(
        f.task_id(),
        &review.bindings,
    )
    .unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(review.bindings[0].receiving_repository.is_dir());
    drop(held);
    worker.turn().unwrap();
    assert!(!review.bindings[0].receiving_repository.exists());
}

#[test]
fn interrupted_directory_removal_resumes_after_ownership_marker_is_gone() {
    let f = Fixture::new();
    let receiver = f.review().bindings[0].receiving_repository.clone();
    let job = deletion::prepare(&f.request.receiver_root, &receiver).unwrap();
    drop(job);
    let tombstone = std::fs::read_dir(&f.request.receiver_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("receiver-deleting")
        })
        .unwrap();
    std::fs::remove_file(tombstone.join(".build-review-receiver.json")).unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(!tombstone.exists());
    assert!(
        !std::fs::read_dir(f.request.receiver_root.join("operations"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("delete-"))
    );
}

#[test]
fn merged_review_does_not_retain_an_otherwise_recoverable_workspace() {
    let f = Fixture::new();
    let mut review = f.review();
    review.pull_request.as_mut().unwrap().status = PullRequestStatus::Merged;
    assert!(!retention::retained(&review).unwrap());
}

#[test]
fn terminal_cleanup_refuses_native_receiving_ref_lock() {
    let f = Fixture::new();
    let review = f.review();
    f.store
        .complete_review(f.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    let binding = &review.bindings[0];
    let lock = binding
        .receiving_repository
        .join(format!("{}.lock", binding.receiving_ref));
    std::fs::write(&lock, "native receiving writer\n").unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    let _ = worker.turn();
    assert!(binding.receiving_repository.is_dir());
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "native receiving writer\n"
    );
    std::fs::remove_file(lock).unwrap();
    worker.turn().unwrap();
    assert!(!binding.receiving_repository.exists());
}

#[test]
fn shared_receiver_releases_only_abandoned_reviews_refs_and_journals() {
    use crate::git_fixture::git_in;
    use crate::reviews::opening::{open, OpenReviewRequest};
    use crate::reviews::sync::reconcile::tests::Hooks;
    let f = Fixture::new();
    let original = f.review();
    let mut workspace = f.request.workspace.clone();
    workspace.id = "workspace-2".into();
    workspace.root = f._home.path().join("second-workspace");
    std::fs::create_dir_all(&workspace.root).unwrap();
    workspace.directories[0].path = workspace.root.join("repo");
    git_in(
        &f.source,
        &[
            "worktree",
            "add",
            "-b",
            "second-work",
            workspace.directories[0].path.to_str().unwrap(),
        ],
    );
    workspace.directories[0].branch = Some("second-work".into());
    crate::isolation::record_branch_teardown(
        &workspace.directories[0].path,
        crate::isolation::BranchTeardown::DeletesBranch,
    )
    .unwrap();
    crate::workspace::persist_review_workspace(&workspace).unwrap();
    let mut request = f.request.request.clone();
    request.workspace_id = workspace.id.clone();
    let second = open(
        &f.store,
        &OpenReviewRequest {
            project_path: f.request.project_path.clone(),
            request_id: "second-review".into(),
            receiver_root: f.request.receiver_root.clone(),
            workspace,
            request,
        },
        &Hooks,
    )
    .unwrap();
    f.store
        .complete_review(f.task_id(), original.version, &Actor::User, "Closed")
        .unwrap();
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    worker.turn().unwrap();
    let repository =
        git2::Repository::open_bare(&original.bindings[0].receiving_repository).unwrap();
    assert!(repository
        .find_reference(&original.bindings[0].receiving_ref)
        .is_err());
    assert!(repository
        .find_reference(&second.review.bindings[0].receiving_ref)
        .is_ok());
    let snapshot = &second.review.snapshots[0];
    assert!(repository
        .find_commit(git2::Oid::from_str(snapshot.directories[0].head.as_ref().unwrap()).unwrap())
        .is_ok());
    use sha2::{Digest, Sha256};
    assert!(!f
        .request
        .receiver_root
        .join("operations")
        .join(format!(
            "{:x}-dispatch.json",
            Sha256::digest(f.task_id().as_bytes())
        ))
        .exists());
}

#[test]
fn owned_orphans_are_deleted_but_foreign_repositories_and_refs_are_preserved() {
    let f = Fixture::new();
    let receiver = f.review().bindings[0].receiving_repository.clone();
    let repository = git2::Repository::open_bare(&receiver).unwrap();
    let head = repository
        .find_reference(&f.review().bindings[0].receiving_ref)
        .unwrap()
        .target()
        .unwrap();
    repository
        .reference("refs/heads/user-work", head, false, "user content")
        .unwrap();
    let review = f.review();
    f.store
        .complete_review(f.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    std::fs::remove_dir_all(&f.request.workspace.root).unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(receiver.is_dir());
    assert!(repository.find_reference("refs/heads/user-work").is_ok());
    repository
        .find_reference("refs/heads/user-work")
        .unwrap()
        .delete()
        .unwrap();
    worker.turn().unwrap();
    assert!(!receiver.exists());
    let foreign = f.request.receiver_root.join("foreign.git");
    git2::Repository::init_bare(&foreign).unwrap();
    assert!(worker.turn().is_err());
    assert!(foreign.is_dir());
}

#[cfg(unix)]
#[test]
fn allocation_report_does_not_claim_shared_source_blocks() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let receiver = directory.path().join("receiver");
    std::fs::create_dir(&receiver).unwrap();
    std::fs::write(&source, vec![b'x'; 4096]).unwrap();
    std::fs::hard_link(&source, receiver.join("shared.pack")).unwrap();
    assert_eq!(size::allocated(&receiver).unwrap(), 0);
    std::fs::remove_file(source).unwrap();
    assert!(size::allocated(&receiver).unwrap() > 0);
}

#[test]
fn receiver_without_any_review_or_pending_opening_is_removed() {
    let (home, source) = crate::git_fixture::init_repo();
    let root = home.path().join("receivers");
    let receiver = crate::reviews::receivers::plan_receiver(&source, &root).unwrap();
    crate::reviews::receivers::ensure_receiver(&receiver).unwrap();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut worker = Worker::new(store, root);
    worker.turn().unwrap();
    assert!(!receiver.path.exists());
}

#[cfg(unix)]
#[test]
fn operations_symlink_is_rejected_before_any_deletion() {
    let f = Fixture::new();
    let receiver = f.review().bindings[0].receiving_repository.clone();
    let operations = f.request.receiver_root.join("operations");
    let moved = f._home.path().join("other-operations");
    std::fs::rename(&operations, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &operations).unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    assert!(worker.turn().is_err());
    assert!(receiver.is_dir());
}

#[test]
fn gc_keeps_recent_unpublished_objects_and_retries_after_git_lock_contention() {
    let f = Fixture::new();
    let receiver = f.review().bindings[0].receiving_repository.clone();
    let repository = git2::Repository::open_bare(&receiver).unwrap();
    let unpublished = repository
        .blob(b"native push object not referenced yet")
        .unwrap();
    let lock = receiver.join("packed-refs.lock");
    std::fs::write(&lock, "native git pack-refs\n").unwrap();
    let mut worker = Worker::new(f.store.clone(), f.request.receiver_root.clone());
    assert!(worker.turn().is_err());
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "native git pack-refs\n"
    );
    std::fs::remove_file(lock).unwrap();
    assert_eq!(worker.turn().unwrap().collected, 1);
    assert_eq!(worker.turn().unwrap().collected, 0);
    assert!(repository.find_blob(unpublished).is_ok());
}

#[test]
fn unfinished_opening_protects_receiver_before_any_review_is_published() {
    use crate::reviews::opening::{open, OpeningHooks, OpeningStep};
    use crate::workspace::Workspace;
    struct Interrupt;
    impl OpeningHooks for Interrupt {
        fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
            Ok(())
        }
        fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
            Ok(())
        }
        fn dispatch_reviewer(&self, _: &crate::tracker::Task, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn checkpoint(&self, step: OpeningStep) -> Result<(), String> {
            if step == OpeningStep::ReceiverCreated {
                Err("interrupted after receiver creation".into())
            } else {
                Ok(())
            }
        }
    }
    let (home, source) = crate::git_fixture::init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = crate::reviews::sync::reconcile::tests::request(home.path(), &source);
    assert!(open(&store, &request, &Interrupt).is_err());
    let receiver =
        crate::reviews::receivers::plan_receiver(&source, &request.receiver_root).unwrap();
    assert!(receiver.path.is_dir());
    let mut worker = Worker::new(store, request.receiver_root);
    worker.turn().unwrap();
    assert!(receiver.path.is_dir(), "failed opening remains resumable");
}
