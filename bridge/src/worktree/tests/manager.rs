use super::super::comparison::worktree_status_line_count;
use crate::git_fixture::{git_in, init_repo};
use crate::isolation::probe::rift_or_skip;
use crate::isolation::Isolation;
use crate::worktree::{Worktree, WorktreeManager};
use std::path::{Path, PathBuf};

pub(super) fn manager(dir: &tempfile::TempDir, repo: &Path) -> WorktreeManager {
    WorktreeManager::new(repo, dir.path().join("worktrees"))
}
/// A bare clone of `repo` wired up as its `origin`.
/// A `feature-x` that exists only on the remote: pushed from a second
/// clone and fetched here, so `origin/feature-x` is known but no local
/// `feature-x` is. Answers the bare origin it was pushed to.
pub(super) fn push_feature_x_from_another_clone(dir: &tempfile::TempDir, repo: &Path) -> PathBuf {
    let origin = bare_origin_of(dir, repo);
    let other = dir.path().join("other");
    git_in(
        dir.path(),
        &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
    );
    git_in(&other, &["config", "user.email", "o@build.ing"]);
    git_in(&other, &["config", "user.name", "O"]);
    git_in(&other, &["checkout", "-b", "feature-x"]);
    std::fs::write(other.join("theirs.txt"), "their work\n").unwrap();
    git_in(&other, &["add", "."]);
    git_in(&other, &["commit", "-m", "their work"]);
    git_in(&other, &["push", "origin", "feature-x"]);
    git_in(repo, &["fetch", "origin"]);
    origin
}
pub(super) fn bare_origin_of(dir: &tempfile::TempDir, repo: &Path) -> PathBuf {
    let origin = dir.path().join("origin.git");
    git_in(
        dir.path(),
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    git_in(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    origin
}
/// The relative path from `from` to `to`, for a test that writes the
/// pointer git itself would write with relative paths turned on.
pub(super) fn pathdiff_from(from: &Path, to: &Path) -> String {
    let from = std::fs::canonicalize(from).unwrap();
    let to = std::fs::canonicalize(to).unwrap();
    let shared = from
        .components()
        .zip(to.components())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from.components().count() - shared;
    let mut path = PathBuf::from("../".repeat(ups).trim_end_matches('/'));
    path.extend(to.components().skip(shared));
    path.display().to_string()
}
#[test]
fn two_worktrees_on_one_repo_are_independent() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);

    let a = mgr
        .create("task-a", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let b = mgr
        .create("task-b", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    assert_ne!(a.path, b.path);
    assert!(a.path.join("README.md").exists());
    assert!(b.path.join("README.md").exists());

    // A change in one worktree's branch does not appear in the other.
    std::fs::write(a.path.join("only-a.txt"), "a").unwrap();
    assert!(!b.path.join("only-a.txt").exists());
}
/// The status count runs its git child through the one runner, so a
/// failure carries what git said and which command said it.
#[test]
fn a_status_count_that_fails_says_which_command_failed() {
    let dir = tempfile::tempdir().unwrap();

    let failure = worktree_status_line_count(dir.path())
        .unwrap_err()
        .to_string();

    assert!(failure.contains("status"), "{failure}");
    assert!(failure.contains("not a git repository"), "{failure}");
}
/// Availability now comes from the probe: a linked worktree is never
/// locked, and a clone is locked exactly when this volume cannot make one,
/// in the probe's own words. The manager owns no reason of its own.
#[test]
fn a_clones_availability_is_the_probes_answer() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let availability = mgr.availability();

    assert_eq!(availability.lock_reason(Isolation::Worktree), None);
    assert_eq!(
        availability.lock_reason(Isolation::Rift),
        availability.rift.as_ref().err().map(String::as_str),
        "the clone lock is exactly the probe's failure sentence"
    );
}
/// Every backend sits at the isolation it makes, in the order the enum
/// names them — and the clone backend arrived in this stage, so no slot is
/// empty and every isolation resolves to a backend.
#[test]
fn each_backend_sits_at_the_isolation_it_makes() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);

    for (isolation, slot) in Isolation::ALL.into_iter().zip(mgr.backends()) {
        let backend = slot.expect("every isolation has a backend in this build");
        assert_eq!(backend.kind(), isolation);
        assert_eq!(mgr.backend(isolation).unwrap().kind(), isolation);
    }
    assert_eq!(
        mgr.backends()
            .map(|slot| slot.map(|backend| backend.kind())),
        [Some(Isolation::Worktree), Some(Isolation::Rift)],
    );
}
/// A clone is a directory, and a directory already under the worktrees root
/// makes its name taken whatever made it — so a second create disambiguates.
#[test]
fn a_clone_directory_makes_its_name_taken() {
    let (dir, repo) = init_repo();
    if !rift_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);

    let first = mgr.create("dup", "main", Isolation::Rift).unwrap().worktree;
    let second = mgr.create("dup", "main", Isolation::Rift).unwrap().worktree;

    assert_eq!(first.name, "dup");
    assert_eq!(
        second.name, "dup-2",
        "the existing clone directory made the name taken"
    );
    assert_ne!(first.path, second.path);
}
/// The state an outside cleanup leaves behind: directory removed,
/// bookkeeping pruned, branch deleted.
pub(super) fn fully_vanished(repo: &Path, wt: &Worktree) {
    std::fs::remove_dir_all(&wt.path).unwrap();
    git_in(repo, &["worktree", "prune"]);
    git_in(repo, &["branch", "-D", &wt.recorded_branch]);
}
#[test]
fn removing_an_already_vanished_worktree_succeeds() {
    // The defect this guards: a Build worktree cleaned up outside Build
    // (dir, bookkeeping AND branch gone) made remove() fail on git2's
    // baffling "could not find '.git/shallow' to stat" from find_worktree,
    // which blocked the plan approve that only wanted the worktree gone.
    // Removal's goal is absence; finding absence is success.
    let (dir, repo) = init_repo();
    let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
    let wt = manager
        .create("gone-slug", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    fully_vanished(&repo, &wt);

    manager.remove(&wt).expect("absence is the goal");
}
/// A linked worktree keeps its record in the project, so a checkout gone
/// from disk is still answered for: the branch it only borrowed stands and
/// the branch it was cut for goes, with nobody having told the removal
/// which was which.
#[test]
fn a_vanished_linked_worktree_still_answers_from_its_record() {
    let (dir, repo) = init_repo();
    let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    let borrowed = manager
        .create_on_existing_branch("theirs", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let cut = manager
        .create("cut-for-me", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&borrowed.path).unwrap();
    std::fs::remove_dir_all(&cut.path).unwrap();

    manager.remove(&borrowed).unwrap();
    manager.remove(&cut).unwrap();

    assert!(
        r.find_branch("theirs", git2::BranchType::Local).is_ok(),
        "the borrowed branch its record kept survives"
    );
    assert!(
        r.find_branch(&cut.recorded_branch, git2::BranchType::Local)
            .is_err(),
        "the branch its record says teardown owns goes"
    );
}
