//! A real rebase changes local committed HEAD without publishing it.
use super::reconcile::tests::Fixture;
use crate::git_fixture::git_in;

#[test]
fn rebasing_committed_workspace_changes_without_push_keeps_received_snapshot() {
    let fixture = Fixture::new();
    fixture.commit("rebase-first.txt");
    let published = fixture.commit("rebase-last.txt");
    fixture.push();
    fixture.sync();
    let saved = fixture.review().snapshots;
    assert_eq!(saved.len(), 2);
    // Replay the last commit onto the original base, omitting the first commit.
    // This changes HEAD through Git's rebase operation without moving the target.
    git_in(
        fixture.checkout(),
        &["rebase", "--onto", "HEAD~2", "HEAD~1"],
    );
    let local = git2::Repository::open(fixture.checkout())
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    assert_ne!(local, published);
    assert!(
        fixture.sync().persisted,
        "the changed working HEAD is observed"
    );
    let review = fixture.review();
    assert_eq!(
        review.snapshots, saved,
        "rebase alone does not publish local HEAD"
    );
    assert_eq!(
        review.snapshots[1].directories[0].head.as_deref(),
        Some(published.as_str())
    );
}
