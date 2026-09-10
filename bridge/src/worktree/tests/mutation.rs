use super::super::mutation::{CreatedLocalRef, PreparedBranch};
use super::command::{commit_file, git_output, tip_of};
use super::manager::{bare_origin_of, manager, pathdiff_from, push_feature_x_from_another_clone};
use crate::git_fixture::{git_in, init_repo, init_repo_named};
use crate::isolation::cow::CowBackend;
use crate::isolation::probe::cow_or_skip;
use crate::isolation::{
    branch_teardown, record_branch_teardown, BranchTeardown, Isolation, IsolationBackend,
    BRANCH_TEARDOWN_MARKER,
};
use crate::worktree::{UnregisteredRestore, Worktree, WorktreeError, WorktreeManager};

/// A name the caller gave is a name, not a description: the branch is cut
/// exactly as asked, and the directory it lands in is derived from it.
#[test]
fn create_cutting_branch_cuts_the_branch_exactly_as_it_was_named() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);

    let prefixed = mgr
        .create_cutting_branch("build/csv-export", "main", Isolation::Worktree)
        .unwrap();
    assert_eq!(prefixed.worktree.recorded_branch, "build/csv-export");
    assert_eq!(prefixed.worktree.name, "csv-export");
    assert_eq!(
        prefixed.teardown,
        BranchTeardown::DeletesBranch,
        "nothing was on that name before"
    );
    assert_eq!(
        branch_teardown(&prefixed.worktree.path).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert!(prefixed.worktree.path.join("README.md").exists());

    // A name with no namespace stays with no namespace: nothing is added to
    // what the caller asked for.
    let plain = mgr
        .create_cutting_branch("hotfix", "main", Isolation::Worktree)
        .unwrap();
    assert_eq!(plain.worktree.recorded_branch, "hotfix");
    assert_eq!(plain.worktree.name, "hotfix");

    // A namespace that is not this manager's is kept whole in the directory
    // name, so two branches never share one directory.
    let foreign = mgr
        .create_cutting_branch("feature/csv-export", "main", Isolation::Worktree)
        .unwrap();
    assert_eq!(foreign.worktree.recorded_branch, "feature/csv-export");
    assert_eq!(foreign.worktree.name, "feature-csv-export");

    let r = git2::Repository::open(&repo).unwrap();
    for branch in ["build/csv-export", "hotfix", "feature/csv-export"] {
        assert!(
            r.find_branch(branch, git2::BranchType::Local).is_ok(),
            "{branch} was cut"
        );
    }
}
/// A branch that already exists is checked out, not cut again — dispatching
/// onto work someone started by hand is the whole point of naming a branch.
#[test]
fn create_on_existing_branch_checks_out_a_branch_that_already_exists() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("build/started-by-hand", &head, false).unwrap();

    let added = mgr
        .create_on_existing_branch("build/started-by-hand", "main", Isolation::Worktree)
        .unwrap();

    assert_eq!(added.worktree.recorded_branch, "build/started-by-hand");
    assert_eq!(
        added.teardown,
        BranchTeardown::KeepsBranch,
        "the branch was already there, and removing this checkout must not take it"
    );
    assert_eq!(
        branch_teardown(&added.worktree.path).unwrap(),
        BranchTeardown::KeepsBranch
    );
    let checkout = git2::Repository::open(&added.worktree.path).unwrap();
    assert_eq!(
        checkout.head().unwrap().shorthand(),
        Some("build/started-by-hand")
    );

    // And teardown reads that answer for itself.
    mgr.remove(&added.worktree).unwrap();
    assert!(r
        .find_branch("build/started-by-hand", git2::BranchType::Local)
        .is_ok());
}
/// A branch only a remote carries is fetched, made local with its upstream
/// set, and checked out — never cut fresh over the top of the work it
/// already holds.
#[test]
fn create_on_existing_branch_materialises_a_branch_only_a_remote_carries() {
    let (dir, repo) = init_repo();
    push_feature_x_from_another_clone(&dir, &repo);
    let mgr = manager(&dir, &repo);

    let added = mgr
        .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
        .unwrap();

    assert_eq!(added.teardown, BranchTeardown::KeepsBranch);
    assert_eq!(
        branch_teardown(&added.worktree.path).unwrap(),
        BranchTeardown::KeepsBranch
    );
    assert_eq!(
        std::fs::read_to_string(added.worktree.path.join("theirs.txt")).unwrap(),
        "their work\n",
        "the checkout carries the work the remote branch already had"
    );
    let r = git2::Repository::open(&repo).unwrap();
    let config = r.config().unwrap();
    assert_eq!(
        config.get_string("branch.feature-x.remote").unwrap(),
        "origin"
    );
    assert_eq!(
        config.get_string("branch.feature-x.merge").unwrap(),
        "refs/heads/feature-x"
    );
}
/// A caller that named a branch meant that branch. When nothing anywhere
/// holds it, the answer is an error — never a fresh empty branch wearing
/// its name.
#[test]
fn create_on_existing_branch_refuses_a_branch_no_ref_holds() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);

    let refused = mgr
        .create_on_existing_branch("nobody-cut-this", "main", Isolation::Worktree)
        .unwrap_err()
        .to_string();

    assert!(refused.contains("nobody-cut-this"), "{refused}");
    let r = git2::Repository::open(&repo).unwrap();
    assert!(r
        .find_branch("nobody-cut-this", git2::BranchType::Local)
        .is_err());
    assert!(!dir.path().join("worktrees/nobody-cut-this").exists());
}
/// The one network-facing step in a checkout: the remote no longer has the
/// branch its tracking ref promised. The error names the remote and the
/// branch, and nothing half-made survives it — no local ref, no directory.
#[test]
fn create_on_existing_branch_surfaces_a_fetch_that_no_longer_carries_the_branch() {
    let (dir, repo) = init_repo();
    let origin = push_feature_x_from_another_clone(&dir, &repo);
    git_in(&origin, &["branch", "-D", "feature-x"]);
    let mgr = manager(&dir, &repo);

    let error = mgr
        .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
        .unwrap_err()
        .to_string();

    assert!(error.contains("origin"), "{error}");
    assert!(error.contains("feature-x"), "{error}");
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("feature-x", git2::BranchType::Local).is_err(),
        "no half-made local ref"
    );
    assert!(
        !dir.path().join("worktrees/feature-x").exists(),
        "no half-made checkout"
    );
}
/// A branch this call cut for itself must not outlive a checkout that
/// never happened: left behind as a local ref, a retry would find it,
/// borrow it, and never let teardown delete it again.
#[test]
fn create_cutting_branch_takes_back_the_branch_it_cut_when_the_checkout_fails() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let worktrees = dir.path().join("worktrees");
    std::fs::create_dir_all(&worktrees).unwrap();
    std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o500)).unwrap();

    let error = mgr
        .create_cutting_branch("fresh-cut", "main", Isolation::Worktree)
        .unwrap_err();

    std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o700)).unwrap();
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("fresh-cut", git2::BranchType::Local).is_err(),
        "the branch it cut was left behind after {error}"
    );
    assert!(!worktrees.join("fresh-cut").exists());
}
/// The same for a branch it made local from a remote: neither the ref nor
/// the tracking config it wrote for it survive a checkout that failed.
#[test]
fn create_on_existing_branch_takes_back_a_branch_it_materialised_when_the_checkout_fails() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, repo) = init_repo();
    push_feature_x_from_another_clone(&dir, &repo);
    let mgr = manager(&dir, &repo);
    let worktrees = dir.path().join("worktrees");
    std::fs::create_dir_all(&worktrees).unwrap();
    std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o500)).unwrap();

    let error = mgr
        .create_on_existing_branch("feature-x", "main", Isolation::Worktree)
        .unwrap_err();

    std::fs::set_permissions(&worktrees, std::fs::Permissions::from_mode(0o700)).unwrap();
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("feature-x", git2::BranchType::Local).is_err(),
        "the branch it materialised was left behind after {error}"
    );
    let config = r.config().unwrap();
    assert!(
        config.get_string("branch.feature-x.remote").is_err(),
        "its tracking config was left behind"
    );
    assert!(
        r.find_reference("refs/remotes/origin/feature-x").is_ok(),
        "the remote-tracking ref was never this call's to take"
    );
}
/// The marker is all that stands between a borrowed branch and the next
/// teardown, so a registration it cannot be written into is taken back.
/// Left behind, that checkout would read as one Build cut the branch for,
/// and a `worktree.finish delete` on it would take somebody else's work.
#[test]
fn a_checkout_whose_teardown_cannot_be_stamped_is_taken_back() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    let added = mgr
        .create_on_existing_branch("theirs", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let marker = repo
        .join(".git/worktrees")
        .join(&added.name)
        .join(BRANCH_TEARDOWN_MARKER);
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o400)).unwrap();

    let borrowed = PreparedBranch {
        teardown: BranchTeardown::KeepsBranch,
        created_ref: None,
    };
    let error = mgr
        .stamp_teardown_or_unwind(&r, "theirs", &added.name, &added.path, &borrowed)
        .unwrap_err();

    assert!(matches!(error, WorktreeError::Io(_)), "{error:?}");
    assert!(
        r.find_branch("theirs", git2::BranchType::Local).is_ok(),
        "the branch it only borrowed survives"
    );
    assert!(
        r.find_worktree(&added.name).is_err(),
        "no registration is left over the branch without a marker"
    );
    assert!(!added.path.exists(), "no half-made checkout");
}
/// The same unwind, for a checkout whose branch this call cut: the branch
/// goes with the registration, so nothing of the failed checkout is left
/// for a retry to mistake for somebody else's work.
#[test]
fn a_checkout_whose_teardown_cannot_be_stamped_takes_the_branch_it_cut_with_it() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let added = mgr
        .create_cutting_branch("fresh-cut", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let marker = repo
        .join(".git/worktrees")
        .join(&added.name)
        .join(BRANCH_TEARDOWN_MARKER);
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o400)).unwrap();

    let cut = PreparedBranch {
        teardown: BranchTeardown::DeletesBranch,
        created_ref: Some(CreatedLocalRef("fresh-cut".to_string())),
    };
    let error = mgr
        .stamp_teardown_or_unwind(&r, "fresh-cut", &added.name, &added.path, &cut)
        .unwrap_err();

    assert!(matches!(error, WorktreeError::Io(_)), "{error:?}");
    assert!(
        r.find_branch("fresh-cut", git2::BranchType::Local).is_err(),
        "the branch it cut was left behind"
    );
    assert!(r.find_worktree(&added.name).is_err());
    assert!(!added.path.exists());
}
/// A checkout Build cut the branch for says so where the fact survives
/// everything but the checkout itself: git's own admin directory for it,
/// which git prunes when the worktree goes.
#[test]
fn a_checkout_build_cut_the_branch_for_says_teardown_owns_it() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("owned", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    assert_eq!(
        branch_teardown(&wt.path).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert!(
        repo.join(".git/worktrees/owned/build-branch-teardown")
            .is_file(),
        "the fact lives in git's admin directory for the checkout"
    );
}
/// A checkout nobody marked is one Build did not create — a worktree made
/// by hand and adopted — and the finish action the human chose on it
/// speaks for its branch, exactly as it did before markers existed.
#[test]
fn an_unmarked_checkout_leaves_its_branch_to_the_action_chosen() {
    let (dir, repo) = init_repo();
    let by_hand = dir.path().join("by-hand");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            by_hand.to_str().unwrap(),
            "-b",
            "made-by-hand",
        ],
    );

    assert_eq!(
        branch_teardown(&by_hand).unwrap(),
        BranchTeardown::DeletesBranch
    );
}
/// Git 2.48+ writes a relative `gitdir:` pointer when
/// `worktree.useRelativePaths` is set. A relative pointer is a valid
/// pointer, resolved against the checkout that holds it.
#[test]
fn branch_teardown_follows_a_relative_gitdir_pointer() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("relative-pointer", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    record_branch_teardown(&wt.path, BranchTeardown::KeepsBranch).unwrap();
    let admin = repo.join(".git/worktrees/relative-pointer");
    let relative = pathdiff_from(&wt.path, &admin);
    std::fs::write(wt.path.join(".git"), format!("gitdir: {relative}\n")).unwrap();

    assert_eq!(
        branch_teardown(&wt.path).unwrap(),
        BranchTeardown::KeepsBranch
    );
}
/// Guessing here deletes somebody's branch, so nothing is guessed: a
/// pointer that resolves to no directory is an error, not an answer.
#[test]
fn branch_teardown_errors_rather_than_guessing_when_it_cannot_read() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("unreadable", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::write(wt.path.join(".git"), "gitdir: /nowhere/at/all\n").unwrap();
    assert!(branch_teardown(&wt.path).is_err());

    std::fs::write(wt.path.join(".git"), "not a pointer at all\n").unwrap();
    assert!(branch_teardown(&wt.path).is_err());

    let admin = repo.join(".git/worktrees/unreadable");
    std::fs::write(admin.join("build-branch-teardown"), "gibberish").unwrap();
    std::fs::write(
        wt.path.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .unwrap();
    assert!(branch_teardown(&wt.path).is_err());

    assert!(branch_teardown(&dir.path().join("no-such-checkout")).is_err());
}
#[test]
fn create_makes_branch_and_working_dir() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);

    let wt = mgr
        .create("fix-typo", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    assert_eq!(wt.recorded_branch, "build/fix-typo");
    assert_eq!(wt.base_branch, "main");
    assert!(wt.path.join("README.md").exists(), "worktree has the files");

    // The branch exists in the repo.
    let r = git2::Repository::open(&repo).unwrap();
    assert!(r
        .find_branch("build/fix-typo", git2::BranchType::Local)
        .is_ok());
    // And the worktree is registered.
    assert!(r.worktrees().unwrap().iter().any(|n| n == Some("fix-typo")));
}
#[test]
fn create_disambiguates_on_collision() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let a = mgr
        .create("dup", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let b = mgr
        .create("dup", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let c = mgr
        .create("dup", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    assert_eq!(a.name, "dup");
    assert_eq!(b.name, "dup-2");
    assert_eq!(c.name, "dup-3");
    assert_eq!(b.recorded_branch, "build/dup-2");
    assert!(b.path.join("README.md").exists());
}
/// An abandoned run's work outlives the run so it can be re-attempted, so
/// its checkout goes and its branch stays — a promise about the branch
/// that the checkout's own teardown marker does not make.
#[test]
fn removing_a_checkout_while_keeping_its_branch_leaves_the_branch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("keep-me", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    mgr.remove_keeping_branch(&wt).unwrap();

    assert!(!wt.path.exists(), "working dir removed");
    let r = git2::Repository::open(&repo).unwrap();
    assert!(!r.worktrees().unwrap().iter().any(|n| n == Some("keep-me")));
    assert!(
        r.find_branch("build/keep-me", git2::BranchType::Local)
            .is_ok(),
        "branch kept"
    );
}
#[test]
fn restore_recreates_the_original_worktree_from_its_local_branch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("recover-local", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::write(wt.path.join("stage.txt"), "kept\n").unwrap();
    git_in(&wt.path, &["add", "stage.txt"]);
    git_in(&wt.path, &["commit", "-m", "stage"]);
    let head = git2::Repository::open(&wt.path)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    git_in(
        &repo,
        &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
    );

    let restored = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap();
    assert_eq!(restored, wt);
    assert_eq!(
        branch_teardown(&restored.path).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert_eq!(
        std::fs::read_to_string(wt.path.join("stage.txt")).unwrap(),
        "kept\n"
    );
    assert_eq!(
        git2::Repository::open(&wt.path)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap(),
        head
    );
}
/// The checks a restore makes on a checkout that is still there, whatever
/// made it: it is on the branch that was recorded for it, and that branch
/// grew out of the base it was cut from.
#[test]
fn restore_refuses_a_checkout_that_left_its_recorded_branch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("wandered", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    git_in(&wt.path, &["checkout", "--detach"]);

    let refused = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap_err()
        .to_string();

    assert!(
        refused.contains("not on the exact persisted branch"),
        "{refused}"
    );
}
#[test]
fn restore_refuses_a_branch_that_shares_no_history_with_its_base() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("unrelated", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let empty_tree = git_output(&repo, &["hash-object", "-t", "tree", "/dev/null"]);
    let orphan = git_output(
        &repo,
        &["commit-tree", empty_tree.trim(), "-m", "unrelated"],
    );
    git_in(
        &repo,
        &[
            "update-ref",
            &format!("refs/heads/{}", wt.recorded_branch),
            orphan.trim(),
        ],
    );

    let refused = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap_err()
        .to_string();

    assert!(refused.contains("no verified ancestry"), "{refused}");
}
/// A checkout deleted outside Build leaves git's record of it behind, and
/// git refuses to add a worktree under a name a record still holds. The
/// record is stale the moment the directory goes, so restore clears it
/// before materializing.
#[test]
fn restore_recreates_a_checkout_deleted_outside_build() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("hand-deleted", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    std::fs::remove_dir_all(&wt.path).unwrap();
    assert!(
        git2::Repository::open(&repo)
            .unwrap()
            .find_worktree(&wt.name)
            .is_ok(),
        "git still records the checkout somebody deleted by hand"
    );

    let restored = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap();

    assert_eq!(restored, wt);
    assert!(wt.path.join("README.md").exists());
}
/// A directory sitting where a checkout belongs, which is no checkout at
/// all, is refused for what it is — nothing ran git to say otherwise.
#[test]
fn restore_rejects_an_existing_unregistered_directory() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let path = dir.path().join("worktrees").join("forged");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("loot"), "not a worktree\n").unwrap();
    let forged = Worktree {
        name: "forged".into(),
        path,
        recorded_branch: "build/forged".into(),
        base_branch: "main".into(),
    };

    let error = mgr
        .restore(
            &forged,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a Build checkout"), "{error}");
}
/// A symlink sitting at the managed path and pointing at a checkout outside
/// the root is not the checkout that was recorded: whatever the path
/// spells, what it resolves to must still be under the root.
#[test]
fn restore_refuses_a_managed_path_that_resolves_outside_the_root() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let elsewhere = dir.path().join("elsewhere");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            elsewhere.to_str().unwrap(),
            "-b",
            "build/escaped",
        ],
    );
    let root = dir.path().join("worktrees");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("escaped");
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    let escaped = Worktree {
        name: "escaped".into(),
        path,
        recorded_branch: "build/escaped".into(),
        base_branch: "main".into(),
    };

    let refused = mgr
        .restore(
            &escaped,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap_err()
        .to_string();

    assert!(refused.contains("canonical managed path"), "{refused}");
}
#[test]
fn restore_fetches_the_original_branch_when_only_origin_has_it() {
    let (dir, repo) = init_repo();
    bare_origin_of(&dir, &repo);
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("recover-remote", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::write(wt.path.join("remote-stage.txt"), "remote\n").unwrap();
    git_in(&wt.path, &["add", "remote-stage.txt"]);
    git_in(&wt.path, &["commit", "-m", "remote stage"]);
    git_in(&wt.path, &["push", "-u", "origin", &wt.recorded_branch]);
    git_in(
        &repo,
        &["worktree", "remove", "--force", wt.path.to_str().unwrap()],
    );
    git_in(&repo, &["branch", "-D", &wt.recorded_branch]);
    git_in(
        &repo,
        &[
            "update-ref",
            "-d",
            &format!("refs/remotes/origin/{}", wt.recorded_branch),
        ],
    );

    mgr.restore(
        &wt,
        UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
        Isolation::Worktree,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(wt.path.join("remote-stage.txt")).unwrap(),
        "remote\n"
    );
    assert_eq!(
        branch_teardown(&wt.path).unwrap(),
        BranchTeardown::DeletesBranch
    );
}
/// A checkout whose directory vanished but whose registration git still
/// holds carries its own answer across the restore: the marker is read out
/// of the admin directory before the prune takes it, and written back into
/// the fresh one. Losing it would hand somebody else's branch to the next
/// teardown.
#[test]
fn restore_carries_the_teardown_marker_a_registration_still_holds() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    let added = mgr
        .create_on_existing_branch("theirs", "main", Isolation::Worktree)
        .unwrap();
    std::fs::remove_dir_all(&added.worktree.path).unwrap();

    let restored = mgr
        .restore(
            &added.worktree,
            UnregisteredRestore::Refuse,
            Isolation::Worktree,
        )
        .unwrap();

    assert_eq!(
        branch_teardown(&restored.path).unwrap(),
        BranchTeardown::KeepsBranch
    );
}
/// With the registration gone, nothing on disk says whose branch this is.
/// A caller that cannot vouch for it gets an error and an untouched
/// repository — no re-added worktree, no fetch, no marker of either value.
#[test]
fn restore_refuses_when_no_registration_and_no_caller_can_vouch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("unvouched", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    mgr.remove(&wt).unwrap();

    let error = mgr
        .restore(&wt, UnregisteredRestore::Refuse, Isolation::Worktree)
        .unwrap_err()
        .to_string();

    assert!(error.contains("unvouched"), "{error}");
    assert!(!wt.path.exists());
    let r = git2::Repository::open(&repo).unwrap();
    assert!(r.find_worktree("unvouched").is_err());
}
/// A registration git cannot read is not a registration that is gone.
/// Only absence lets the caller vouch for the branch; every other git
/// failure is surfaced, because guessing here hands somebody's branch to
/// the next teardown.
#[test]
fn restore_surfaces_a_registration_it_cannot_read_instead_of_guessing() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    let wt = mgr
        .create_on_existing_branch("theirs", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&wt.path).unwrap();
    git_in(&repo, &["update-ref", "-d", "refs/heads/theirs"]);
    std::fs::remove_file(repo.join(".git/worktrees/theirs/gitdir")).unwrap();

    let error = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Worktree,
        )
        .unwrap_err();

    assert!(
        matches!(error, WorktreeError::Git(_)),
        "the unreadable registration is the answer, not a fetch: {error:?}"
    );
    assert!(!wt.path.exists(), "nothing was re-added");
}
/// A checkout Build cut a branch for takes that branch with it, and no
/// caller has to say so — the checkout does.
#[test]
fn removing_a_checkout_build_cut_takes_its_branch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("drop-me", "main", Isolation::Worktree)
        .unwrap()
        .worktree;

    mgr.remove(&wt).unwrap();

    assert!(!wt.path.exists());
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("build/drop-me", git2::BranchType::Local)
            .is_err(),
        "branch deleted"
    );
}
#[test]
fn a_bound_path_excludes_its_checkout_in_whatever_spelling_it_arrives() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-bound");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "bound/spelled-otherwise",
        ],
    );
    let another_spelling = dir.path().join("wt-bound-link");
    std::os::unix::fs::symlink(&wt_path, &another_spelling).unwrap();

    let mut excluded = std::collections::HashSet::new();
    excluded.insert(another_spelling);
    let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

    assert!(
        found.is_empty(),
        "the scan canonicalizes what it is told to exclude, so no caller has to: {found:?}"
    );
}
/// Removal's goal is absence: a checkout already gone still takes git's
/// record of it with it, and a kept branch is left whole.
#[test]
fn removing_a_checkout_that_is_already_gone_still_clears_its_record() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("vanished", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&wt.path).unwrap();

    mgr.remove_keeping_branch(&wt).unwrap();

    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        !r.worktrees().unwrap().iter().any(|n| n == Some("vanished")),
        "the record went with the directory"
    );
    assert!(
        r.find_branch(&wt.recorded_branch, git2::BranchType::Local)
            .is_ok(),
        "the kept branch is untouched"
    );
}
/// A checkout is known by its directory, so renaming one leaves git's
/// registry naming something that is not there: removal takes the directory
/// it was pointed at, and the sweep clears what the rename stranded.
#[test]
fn a_checkout_renamed_after_registration_is_removed_by_its_directory() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("was-here", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let renamed = dir.path().join("worktrees").join("now-here");
    std::fs::rename(&wt.path, &renamed).unwrap();

    mgr.remove_checkout(&renamed).unwrap();

    assert!(!renamed.exists(), "the directory it was pointed at is gone");
    let stale = git2::Repository::open(&repo).unwrap();
    assert!(
        stale
            .worktrees()
            .unwrap()
            .iter()
            .any(|n| n == Some("was-here")),
        "the rename stranded the record"
    );

    mgr.prune();

    let swept = git2::Repository::open(&repo).unwrap();
    assert!(
        !swept
            .worktrees()
            .unwrap()
            .iter()
            .any(|n| n == Some("was-here")),
        "the sweep clears it"
    );
}
/// Deleting a branch by an expected head is how a teardown promises it took
/// only what it read; a branch that moved since keeps its work.
#[test]
fn a_branch_is_deleted_only_while_it_still_points_where_it_was_read() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("ref-ops", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    commit_file(&wt.path, "moved-on");
    let branch_tip = tip_of(&repo, &format!("refs/heads/{}", wt.recorded_branch));
    let base_tip = tip_of(&repo, "refs/heads/main");
    mgr.remove_checkout(&wt.path).unwrap();

    assert!(mgr.branch_exists(&wt.recorded_branch).unwrap());
    assert!(!mgr.branch_exists("build/never-cut").unwrap());

    assert!(
        mgr.delete_branch_at(&wt.recorded_branch, &base_tip)
            .is_err(),
        "a branch that is not where it was read keeps its work"
    );
    assert!(mgr.branch_exists(&wt.recorded_branch).unwrap());

    mgr.delete_branch_at(&wt.recorded_branch, &branch_tip)
        .unwrap();
    assert!(!mgr.branch_exists(&wt.recorded_branch).unwrap());

    mgr.restore_branch(&wt.recorded_branch, &branch_tip)
        .unwrap();
    assert_eq!(
        tip_of(&repo, &format!("refs/heads/{}", wt.recorded_branch)),
        branch_tip,
        "the undo puts it back exactly where it was"
    );
}
/// The whole of a clone create through the façade: the branch is cut in the
/// project repo, the working directory is a copy-on-write clone on that
/// branch, and `Isolation::of` reads it back as a clone.
#[test]
fn create_materializes_a_clone_end_to_end() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);

    let wt = mgr
        .create("cloned", "main", Isolation::Cow)
        .unwrap()
        .worktree;

    assert_eq!(wt.recorded_branch, "build/cloned");
    assert_eq!(wt.base_branch, "main");
    assert_eq!(Isolation::of(&wt.path), Some(Isolation::Cow));
    assert!(wt.path.join("README.md").exists(), "the clone is warm");
    let r = git2::Repository::open(&repo).unwrap();
    assert!(
        r.find_branch("build/cloned", git2::BranchType::Local)
            .is_ok(),
        "the branch was cut in the project repo"
    );
    assert_eq!(
        git2::Repository::open(&wt.path)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some("build/cloned"),
        "the clone is on the cut branch"
    );
}
/// A named branch that already exists is checked out into a clone, not cut
/// again — dispatching a clone onto work started by hand reaches it.
#[test]
fn create_on_branch_clones_onto_an_existing_branch() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("build/started-by-hand", &head, false).unwrap();

    let added = mgr
        .create_on_existing_branch("build/started-by-hand", "main", Isolation::Cow)
        .unwrap();

    assert_eq!(
        added.teardown,
        BranchTeardown::KeepsBranch,
        "the branch was already there, not cut again"
    );
    assert_eq!(Isolation::of(&added.worktree.path), Some(Isolation::Cow));
    assert_eq!(
        git2::Repository::open(&added.worktree.path)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some("build/started-by-hand"),
    );
}
/// A clone deleted from disk is recreated on its recorded branch as a
/// clone: the caller's resolved isolation says what to recreate it as, and
/// the branch (published on removal) says where.
#[test]
fn restore_recreates_a_deleted_clone_on_its_recorded_branch() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("recover-clone", "main", Isolation::Cow)
        .unwrap()
        .worktree;
    commit_file(&wt.path, "clone-stage");
    mgr.remove_keeping_branch(&wt).unwrap();
    assert!(!wt.path.exists(), "the clone was removed");

    let restored = mgr
        .restore(
            &wt,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Cow,
        )
        .unwrap();

    assert_eq!(restored, wt);
    assert_eq!(Isolation::of(&wt.path), Some(Isolation::Cow));
    assert!(
        wt.path.join("clone-stage.txt").exists(),
        "the recreated clone carries the published work"
    );
}
/// An existing clone passes restore's verify; a clone whose marker names a
/// different project is refused for what it is — it is not this project's.
#[test]
fn restore_verifies_a_clone_and_rejects_one_of_another_project() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);

    let mine = mgr.create("mine", "main", Isolation::Cow).unwrap().worktree;
    assert_eq!(
        mgr.restore(
            &mine,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Cow
        )
        .unwrap(),
        mine
    );

    let other = init_repo_named(dir.path(), "other");
    let intruder_path = dir.path().join("worktrees").join("intruder");
    CowBackend
        .materialize(&other, "main", &intruder_path)
        .unwrap();
    let intruder = Worktree {
        name: "intruder".into(),
        path: intruder_path,
        recorded_branch: "main".into(),
        base_branch: "main".into(),
    };

    let refused = mgr
        .restore(
            &intruder,
            UnregisteredRestore::Write(BranchTeardown::DeletesBranch),
            Isolation::Cow,
        )
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("not a copy-on-write clone of this project"),
        "{refused}"
    );
}
/// A branch named in full that no ref holds is cut and cloned onto, the
/// third creator answering in a clone like the other two.
#[test]
fn create_cutting_branch_clones_onto_the_branch_it_cut() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);

    let added = mgr
        .create_cutting_branch("hotfix-login", "main", Isolation::Cow)
        .unwrap();

    assert_eq!(added.teardown, BranchTeardown::DeletesBranch);
    assert_eq!(Isolation::of(&added.worktree.path), Some(Isolation::Cow));
    assert_eq!(
        git2::Repository::open(&added.worktree.path)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some("hotfix-login"),
        "the clone is on the branch that was cut exactly as it was named"
    );
}
/// A clone keeps its own `.git`, so what teardown owns is written there
/// beside the marker that says the clone is Build's — and read back from
/// the clone itself, by the same call a linked worktree answers.
#[test]
fn a_clones_teardown_marker_round_trips_in_its_own_git_directory() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();

    let cut = mgr.create("cut-for-me", "main", Isolation::Cow).unwrap();
    let borrowed = mgr
        .create_on_existing_branch("theirs", "main", Isolation::Cow)
        .unwrap();

    assert!(
        cut.worktree
            .path
            .join(".git")
            .join(BRANCH_TEARDOWN_MARKER)
            .exists(),
        "the marker is in the clone's own git directory"
    );
    assert_eq!(
        branch_teardown(&cut.worktree.path).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert_eq!(
        branch_teardown(&borrowed.worktree.path).unwrap(),
        BranchTeardown::KeepsBranch
    );
}
/// A path that never held a checkout is absent already, and absence is
/// success for every backend — the clone backend included.
#[test]
fn remove_checkout_on_a_missing_path_succeeds() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let ghost = dir.path().join("worktrees").join("never-existed");

    mgr.remove_checkout(&ghost)
        .expect("absence is success for every backend");
}
/// A directory that is not a checkout is still a directory the teardown was
/// pointed at: there is nothing to publish from, and it goes.
#[test]
fn removing_a_directory_that_is_no_longer_a_checkout_still_clears_it() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("half-made", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_file(wt.path.join(".git")).unwrap();
    assert_eq!(Isolation::of(&wt.path), None, "no longer a checkout");

    mgr.remove_keeping_branch(&wt)
        .expect("a directory nobody can publish from is still removable");

    assert!(!wt.path.exists(), "the directory is gone");
    assert!(
        git2::Repository::open(&repo)
            .unwrap()
            .find_branch(&wt.recorded_branch, git2::BranchType::Local)
            .is_ok(),
        "the kept branch is untouched"
    );
}
#[test]
fn removing_a_vanished_worktree_leaves_a_branch_it_can_no_longer_vouch_for() {
    // Partial carcass: dir and bookkeeping gone, branch still there. The
    // bookkeeping is where the checkout recorded whose branch that is, so
    // with it gone the branch is reported and left alone rather than
    // deleted on a guess.
    let (dir, repo) = init_repo();
    let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
    let wt = manager
        .create("half-gone", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&wt.path).unwrap();
    git_in(&repo, &["worktree", "prune"]);

    let error = manager.remove(&wt).unwrap_err();

    assert!(
        format!("{error}").contains("half-gone"),
        "the error names the checkout that can no longer answer: {error}"
    );
    let repo = git2::Repository::open(&repo).unwrap();
    assert!(
        repo.find_branch(&wt.recorded_branch, git2::BranchType::Local)
            .is_ok(),
        "a branch nothing can vouch for is left standing"
    );
}
/// The marker decides whether the branch goes, so it is read before
/// anything is destroyed: a checkout that cannot answer is left whole —
/// directory, registration and branch — rather than torn down under a
/// question nothing can be asked again afterwards.
#[test]
fn removing_a_checkout_that_cannot_answer_destroys_nothing() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, repo) = init_repo();
    let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
    let r = git2::Repository::open(&repo).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    let wt = manager
        .create_on_existing_branch("theirs", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let admin_dir = repo.join(".git/worktrees").join(&wt.name);
    std::fs::set_permissions(&admin_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    let error = manager.remove(&wt).unwrap_err();

    std::fs::set_permissions(&admin_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(error, WorktreeError::Io(_)),
        "the unreadable marker is the answer: {error:?}"
    );
    assert!(wt.path.exists(), "the checkout is left standing");
    assert!(
        r.find_worktree(&wt.name).is_ok(),
        "and so is its registration"
    );
    assert!(
        r.find_branch("theirs", git2::BranchType::Local).is_ok(),
        "and the branch it was only borrowing"
    );
}
/// A clone that is gone from disk has taken the only record of it with it.
/// The project can vouch for nothing, so the removal refuses in its own
/// words — no git command ran — and the branch is left standing.
#[test]
fn a_vanished_clone_cannot_vouch_and_its_branch_stands() {
    let (dir, repo) = init_repo();
    if !cow_or_skip(dir.path()) {
        return;
    }
    let manager = WorktreeManager::new(&repo, dir.path().join("wts"));
    let wt = manager
        .create("cloned-away", "main", Isolation::Cow)
        .unwrap()
        .worktree;
    std::fs::remove_dir_all(&wt.path).unwrap();

    let error = manager.remove(&wt).unwrap_err();

    assert!(
        matches!(error, WorktreeError::Refused(_)),
        "nothing ran git to say otherwise: {error:?}"
    );
    assert!(format!("{error}").contains("cloned-away"), "{error}");
    assert!(
        git2::Repository::open(&repo)
            .unwrap()
            .find_branch(&wt.recorded_branch, git2::BranchType::Local)
            .is_ok(),
        "a branch nothing can vouch for is left standing"
    );
}
#[test]
fn removal_refuses_before_deleting_when_the_project_repo_is_unreachable() {
    // A teardown that cannot reach git's registry would delete the
    // directory and strand a record naming it; it must refuse while
    // nothing is lost yet.
    let (dir, repo) = init_repo();
    let root = dir.path().join("wts");
    let wt = WorktreeManager::new(&repo, &root)
        .create("kept-slug", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    let unreachable = WorktreeManager::new(dir.path().join("not-a-repo"), &root);

    unreachable
        .remove_checkout(&wt.path)
        .expect_err("no registry to clear, so nothing is deleted");
    assert!(wt.path.exists(), "the checkout survives a refused teardown");
}
