use super::SurfaceRevision;

#[test]
fn a_bump_moves_the_counter_a_watcher_reads() {
    let revision = SurfaceRevision::default();
    let mut watched = revision.subscribe();

    assert_eq!(*watched.borrow_and_update(), 0);
    revision.bump();
    revision.bump();

    assert_eq!(*watched.borrow_and_update(), 2);
}

#[test]
fn a_clone_of_the_revision_bumps_the_one_counter() {
    let revision = SurfaceRevision::default();
    let held_by_the_reader = revision.clone();

    held_by_the_reader.bump();

    assert_eq!(*revision.subscribe().borrow(), 1);
}

#[test]
fn a_bump_with_nobody_watching_is_not_an_error() {
    let revision = SurfaceRevision::default();
    drop(revision.subscribe());

    revision.bump();
    revision.bump();

    assert_eq!(*revision.subscribe().borrow(), 2);
}
