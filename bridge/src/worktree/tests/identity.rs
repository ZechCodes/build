use super::manager::manager;
use crate::git_fixture::init_repo;
use crate::isolation::Isolation;
use crate::worktree::{derive_adoption_goal, is_ref_name, is_usable_branch_name, slugify};

/// Git's own branch-name rule, which is wider than the one Build cuts
/// under: anything `git branch` would accept is a name git can hold a
/// branch under, and so a name a checkout can be asked for.
#[test]
fn a_ref_name_is_one_git_branch_would_accept() {
    for name in [
        "build/csv-export",
        "wip@2",
        "feature/foo+bar",
        "release-1.2",
        "ünïcode",
    ] {
        assert!(is_ref_name(name), "{name:?} is a name git would take");
    }
    for name in [
        "",
        "HEAD",
        "-dashed",
        "add a csv export",
        "build/",
        "build//x",
        "build/..",
        "back\\slash",
        "star*",
        "tilde~1",
        "at@{brace}",
    ] {
        assert!(!is_ref_name(name), "{name:?} is not a branch name");
    }
}
#[test]
fn a_usable_branch_name_is_one_git_and_the_filesystem_both_take() {
    for name in [
        "build/csv-export",
        "csv-export",
        "feature/api/v2",
        "release-1.2",
        "fix_the_thing",
    ] {
        assert!(is_usable_branch_name(name), "{name:?} is a branch name");
    }
    for name in [
        "",
        "add a csv export",
        "Add CSV export, please",
        "build/",
        "/build",
        "build//x",
        "-dashed",
        ".hidden",
        "build/..",
        "build/x.lock",
        "back\\slash",
        "star*",
        "tilde~1",
    ] {
        assert!(
            !is_usable_branch_name(name),
            "{name:?} is a description, not a branch name"
        );
    }
}
#[test]
fn create_cutting_branch_refuses_a_name_that_is_not_a_branch_name() {
    let (dir, repo) = init_repo();
    let mgr = manager(&dir, &repo);
    let refused = mgr.create_cutting_branch("add a csv export", "main", Isolation::Worktree);
    assert!(refused.is_err(), "{refused:?}");
}
#[test]
fn slugify_is_branch_safe() {
    assert_eq!(
        slugify("Fix the typo in the README"),
        "fix-the-typo-in-the-readme"
    );
    assert_eq!(slugify("  Add OAuth!! support  "), "add-oauth-support");
    assert_eq!(slugify("***"), "task");
    assert_eq!(slugify(""), "task");
    assert!(slugify(&"x".repeat(200)).len() <= 50);
    assert!(!slugify("trailing punctuation...").ends_with('-'));
}
#[test]
fn derive_adoption_goal_pinned_cases() {
    assert_eq!(
        derive_adoption_goal("hotfix/login-redirect", "irrelevant"),
        "hotfix/login-redirect"
    );
    assert_eq!(
        derive_adoption_goal("wip", "Fix the thing"),
        "Fix the thing"
    );
    assert_eq!(
        derive_adoption_goal("wip-2", "some subject"),
        "some subject"
    );
    assert_eq!(
        derive_adoption_goal("ada/test_3", "some subject"),
        "some subject"
    );
    assert_eq!(
        derive_adoption_goal("feature", "some subject"),
        "some subject"
    );
    assert_eq!(derive_adoption_goal("", ""), "Adopted worktree");
}
