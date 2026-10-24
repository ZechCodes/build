use super::*;
use crate::git_fixture::git_in;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::tracker::Actor;

#[test]
fn interrupted_capture_keeps_terminal_receiver_until_recovery_finishes() {
    use crate::reviews::sync::reconcile::recovery::CaptureJournal;
    let fixture = Fixture::new();
    let review = fixture.review();
    let receiver = review.bindings[0].receiving_repository.clone();
    let mut capture = CaptureJournal::acquire(fixture.task_id(), &review.bindings).unwrap();
    capture
        .begin(&fixture.store, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    drop(capture);
    fixture
        .store
        .complete_review(fixture.task_id(), review.version, &Actor::User, "Closed")
        .unwrap();
    std::fs::remove_dir_all(&fixture.request.workspace.root).unwrap();
    let mut worker = Worker::new(fixture.store.clone(), fixture.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(receiver.exists(), "durable recovery must keep its receiver");

    let recovery = CaptureJournal::acquire(fixture.task_id(), &review.bindings).unwrap();
    recovery.recover(&fixture.store, &review.bindings).unwrap();
    drop(recovery);
    worker.turn().unwrap();
    assert!(!receiver.exists());
}

#[test]
fn pushed_review_survives_source_gc_then_closes_and_reclaims_its_receiver() {
    let fixture = Fixture::new();
    let head = fixture.commit("workflow.txt");
    fixture.push();
    fixture.sync();
    let review = fixture.review();
    let receiver = review.bindings[0].receiving_repository.clone();

    std::fs::remove_dir_all(&fixture.request.workspace.root).unwrap();
    git_in(&fixture.source, &["worktree", "prune", "--expire", "now"]);
    let source = git2::Repository::open(&fixture.source).unwrap();
    let oid = git2::Oid::from_str(&head).unwrap();
    // Native pushes also advance remote-tracking refs in the shared source DB.
    // Remove every source ref to the disposable review tip before collecting it.
    let mut refs: Vec<_> = source
        .references()
        .unwrap()
        .map(Result::unwrap)
        .filter(|reference| reference.target() == Some(oid))
        .collect();
    for reference in &mut refs {
        reference.delete().unwrap();
    }
    drop(refs);
    drop(source);
    git_in(
        &fixture.source,
        &["reflog", "expire", "--expire=now", "--all"],
    );
    git_in(&fixture.source, &["gc", "--prune=now"]);
    assert!(git2::Repository::open(&fixture.source)
        .unwrap()
        .find_commit(oid)
        .is_err());

    let receiving = git2::Repository::open_bare(&receiver).unwrap();
    let tree = receiving.find_commit(oid).unwrap().tree().unwrap();
    let object = tree
        .get_name("workflow.txt")
        .unwrap()
        .to_object(&receiving)
        .unwrap();
    assert_eq!(object.as_blob().unwrap().content(), b"workflow.txt\n");
    git_in(&receiver, &["fsck", "--full"]);

    fixture
        .store
        .complete_review(fixture.task_id(), review.version, &Actor::User, "Abandoned")
        .unwrap();
    let mut worker = Worker::new(fixture.store.clone(), fixture.request.receiver_root.clone());
    worker.turn().unwrap();
    assert!(!receiver.exists());
}
