use super::command::commit_file;
use super::manager::manager;
use crate::git_fixture::{git_in, init_repo};
use crate::isolation::probe::rift_or_skip;
use crate::isolation::Isolation;
use crate::worktree::{
    describe_checkout, external_worktree_id, repository_branch_holder, unix_now, ExternalWorktree,
};
use std::collections::HashSet;
use std::path::PathBuf;

#[test]
fn external_worktree_id_is_stable_and_prefixed() {
    let a = PathBuf::from("/Users/zech/Projects/8ly/Build");
    let b = PathBuf::from("/Users/zech/Projects/8ly/Build-hotfix");

    let id_a1 = external_worktree_id(&a);
    let id_a2 = external_worktree_id(&a);
    let id_b = external_worktree_id(&b);

    assert_eq!(id_a1, id_a2);
    assert_ne!(id_a1, id_b);
    assert!(id_a1.starts_with("wt-"));
    assert_eq!(id_a1.len(), 15);
}
/// The listing stamps a row with two facts about the project's repository —
/// the checkout id it hashes to and the branch it holds — and pays for
/// nothing else: no status walk, no diffstat, no subprocess.
#[test]
fn repository_branch_holder_names_the_branch_by_the_checkouts_own_id() {
    let (_dir, repo) = init_repo();

    let holder = repository_branch_holder(&repo).unwrap();

    let id = external_worktree_id(&std::fs::canonicalize(&repo).unwrap());
    assert_eq!(holder, Some((id, "main".to_string())));
}
#[test]
fn repository_branch_holder_is_none_when_head_is_detached() {
    let (_dir, repo) = init_repo();
    git_in(&repo, &["checkout", "--detach"]);

    assert_eq!(repository_branch_holder(&repo).unwrap(), None);
}
/// `Ok(None)` is reserved for a repository that really has no working
/// tree. A path git cannot read as a repository at all is broken, and
/// saying so is what keeps a caller from reading "nothing holds this
/// branch" off a repository that answered nothing.
#[test]
fn repository_branch_holder_errors_on_a_directory_that_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let not_a_repo = dir.path().join("plain");
    std::fs::create_dir(&not_a_repo).unwrap();

    assert!(repository_branch_holder(&not_a_repo).is_err());
}
#[test]
fn discovery_lists_a_user_worktree_and_skips_the_primary() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-a");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "hotfix/thing",
        ],
    );
    std::fs::write(wt_path.join("dirty.txt"), "dirty\n").unwrap();

    let excluded = std::collections::HashSet::new();
    let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

    assert_eq!(found.len(), 1);
    let entry = &found[0];
    assert_eq!(entry.branch, Some("hotfix/thing".to_string()));
    assert_eq!(entry.dirty_files, 1);
    assert!(!entry.head_subject.is_empty());
    assert_eq!(entry.name, "wt-a");
    assert!(entry.id.starts_with("wt-"));

    let primary_canonical = std::fs::canonicalize(&repo).unwrap();
    assert!(found.iter().all(|w| w.path != primary_canonical));
}
#[test]
fn one_checkout_describes_itself_the_way_the_scan_describes_it() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-one");
    git_in(
        &repo,
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", "solo"],
    );
    std::fs::write(wt_path.join("dirty.txt"), "dirty\n").unwrap();

    let scanned = manager(&dir, &repo)
        .discover("main", &HashSet::new())
        .unwrap();
    let described = describe_checkout(&wt_path, "main", unix_now()).unwrap();

    // `head_age_seconds` is a reading of the clock, not a property of the
    // checkout: two reads straddling a second boundary differ by one.
    let described = ExternalWorktree {
        head_age_seconds: scanned[0].head_age_seconds,
        ..described
    };
    assert_eq!(
        described, scanned[0],
        "a checkout described on its own must be the entry a scan would have found"
    );
}
#[test]
fn a_checkout_outside_the_repository_cannot_be_described() {
    let (dir, _repo) = init_repo();
    let stranger = dir.path().join("not-a-worktree");
    std::fs::create_dir_all(&stranger).unwrap();

    let described = describe_checkout(&stranger, "main", unix_now());

    assert!(described.is_none(), "{described:?}");
}
#[test]
fn discovery_separates_what_is_uncommitted_from_what_the_branch_carries() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-mixed");
    git_in(
        &repo,
        &["worktree", "add", wt_path.to_str().unwrap(), "-b", "mixed"],
    );
    // One committed line, then two uncommitted ones on top of it.
    std::fs::write(wt_path.join("committed.txt"), "one\n").unwrap();
    git_in(&wt_path, &["add", "committed.txt"]);
    git_in(&wt_path, &["commit", "-m", "committed work"]);
    std::fs::write(wt_path.join("dirty.txt"), "two\nthree\n").unwrap();

    let found = manager(&dir, &repo)
        .discover("main", &HashSet::new())
        .unwrap();

    let entry = &found[0];
    // The branch delta carries both; the uncommitted stat only what is
    // sitting in the tree unsaved.
    assert_eq!(entry.diffstat.insertions, 3);
    assert_eq!(entry.uncommitted.insertions, 2);
    assert_eq!(entry.uncommitted.files_changed, 1);
}
#[test]
fn discovery_excludes_bound_paths() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-bound");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "bound/thing",
        ],
    );

    let mut excluded = std::collections::HashSet::new();
    excluded.insert(std::fs::canonicalize(&wt_path).unwrap());
    let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

    assert!(found.is_empty());
}
#[test]
fn discovery_reports_detached_head() {
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-d");
    git_in(
        &repo,
        &["worktree", "add", "--detach", wt_path.to_str().unwrap()],
    );

    let excluded = std::collections::HashSet::new();
    let found = manager(&dir, &repo).discover("main", &excluded).unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].branch, None);
}
#[test]
fn discovery_cache_invalidation_sees_new_head() {
    // Not a cache test (Layer 1 owns no cache) — confirms a fresh scan after
    // a new commit reflects the moved HEAD, the property the app-layer cache
    // invalidation relies on.
    let (dir, repo) = init_repo();
    let wt_path = dir.path().join("wt-c");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            "feature/thing",
        ],
    );
    let excluded = std::collections::HashSet::new();
    let before = manager(&dir, &repo).discover("main", &excluded).unwrap();
    let sha_before = before[0].head_sha.clone();

    std::fs::write(wt_path.join("more.txt"), "more\n").unwrap();
    git_in(&wt_path, &["add", "more.txt"]);
    git_in(&wt_path, &["commit", "-m", "more work"]);

    let after = manager(&dir, &repo).discover("main", &excluded).unwrap();
    assert_ne!(before[0].head_sha, after[0].head_sha);
    assert_ne!(sha_before, after[0].head_sha);
}
/// The scan is the union of every backend's, minus the project's own
/// checkout and the paths the caller has already bound, and each entry says
/// how it is isolated.
#[test]
fn discover_lists_every_checkout_but_the_project_and_the_excluded() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let listed = dir.path().join("wt-listed");
    let bound = dir.path().join("wt-bound");
    git_in(
        &repo,
        &["worktree", "add", listed.to_str().unwrap(), "-b", "listed"],
    );
    git_in(
        &repo,
        &["worktree", "add", bound.to_str().unwrap(), "-b", "bound"],
    );
    let mut excluded = HashSet::new();
    excluded.insert(std::fs::canonicalize(&bound).unwrap());

    let found = mgr.discover("main", &excluded).unwrap();

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].name, "wt-listed");
    assert_eq!(found[0].branch.as_deref(), Some("listed"));
    assert_eq!(found[0].isolation, Isolation::Worktree);
}
/// The scan is the union of both backends' walks: a clone and a linked
/// worktree of the same project both surface, each saying what it is, and
/// each is base-synced first — for a clone that is a real fetch from the
/// project, so a base that moved after the clone was made is visible in
/// its counts.
#[test]
fn discover_lists_a_clone_and_a_linked_worktree_of_the_same_project() {
    let (dir, repo) = init_repo();
    if !rift_or_skip(dir.path()) {
        return;
    }
    let mgr = manager(&dir, &repo);
    let clone = mgr
        .create("cloned", "main", Isolation::Rift)
        .unwrap()
        .worktree;
    let linked = dir.path().join("worktrees").join("wt-linked");
    git_in(
        &repo,
        &["worktree", "add", linked.to_str().unwrap(), "-b", "linked"],
    );
    commit_file(&repo, "moved-base");

    let found = mgr.discover("main", &HashSet::new()).unwrap();

    let isolations: Vec<(String, Isolation)> = found
        .iter()
        .map(|checkout| (checkout.name.clone(), checkout.isolation))
        .collect();
    assert_eq!(found.len(), 2, "{isolations:?}");
    assert!(
        isolations.contains(&("cloned".to_string(), Isolation::Rift)),
        "{isolations:?}"
    );
    assert!(
        isolations.contains(&("wt-linked".to_string(), Isolation::Worktree)),
        "{isolations:?}"
    );
    let cloned = found
        .iter()
        .find(|checkout| checkout.path == std::fs::canonicalize(&clone.path).unwrap())
        .expect("the clone is on the board");
    assert_eq!(
        cloned.behind,
        Some(1),
        "the clone's row does not reflect the base sync the scan ran"
    );
}
/// One broken stray must not fail the scan. A checkout git still lists but
/// that is no Build checkout any more is refused a base sync and described
/// by nobody; one git itself gives up on is never listed; and the healthy
/// one beside them is found all the same.
#[test]
fn discover_skips_a_broken_stray_and_keeps_the_rest() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let healthy = dir.path().join("wt-healthy");
    let hollowed = dir.path().join("wt-hollowed");
    let pointerless = dir.path().join("wt-pointerless");
    for (path, branch) in [
        (&healthy, "healthy"),
        (&hollowed, "hollowed"),
        (&pointerless, "pointerless"),
    ] {
        git_in(
            &repo,
            &["worktree", "add", path.to_str().unwrap(), "-b", branch],
        );
    }
    std::fs::remove_file(hollowed.join(".git")).unwrap();
    std::fs::create_dir(hollowed.join(".git")).unwrap();
    std::fs::remove_file(pointerless.join(".git")).unwrap();

    let found = mgr.discover("main", &HashSet::new()).unwrap();

    assert_eq!(
        found.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["wt-healthy"],
        "{found:?}"
    );
}
/// A checkout describes itself, branch and all — and a directory that is no
/// Build checkout is described by nobody.
#[test]
fn a_detached_checkout_is_described_without_a_branch() {
    let (dir, repo) = init_repo();
    let detached = dir.path().join("wt-detached");
    git_in(
        &repo,
        &["worktree", "add", "--detach", detached.to_str().unwrap()],
    );

    let described = describe_checkout(&detached, "main", unix_now())
        .expect("a linked worktree describes itself");

    assert_eq!(described.branch, None);
    assert_eq!(described.name, "wt-detached");
    assert_eq!(described.isolation, Isolation::Worktree);
    assert!(
        describe_checkout(&repo, "main", unix_now()).is_none(),
        "the project's own checkout is nobody's isolated copy"
    );
}
/// The merge runs through the project's own checkout, which must be on the
/// base branch — merging into whatever is at HEAD would land the work
/// somewhere nobody asked for.
#[test]
fn merge_into_base_refuses_a_primary_checkout_on_another_branch() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let wt = mgr
        .create("mergeable", "main", Isolation::Worktree)
        .unwrap()
        .worktree;
    commit_file(&wt.path, "landed");
    git_in(&repo, &["checkout", "-b", "elsewhere"]);

    let refused = mgr
        .merge_into_base(&wt.path, &wt.recorded_branch, "main")
        .unwrap_err()
        .to_string();

    assert!(refused.contains("not the base branch"), "{refused}");
    assert!(
        !refused.contains("git command failed"),
        "the merge never ran, so the refusal must not read as git's failure: {refused}"
    );
    git_in(&repo, &["checkout", "main"]);
    mgr.merge_into_base(&wt.path, &wt.recorded_branch, "main")
        .unwrap();
    assert!(
        repo.join("landed.txt").exists(),
        "the work is on the base branch"
    );
}
