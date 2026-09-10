// Exact test bodies moved from the former inline test module.
use super::*;

/// A branch the repository holds itself is local, whatever its remotes
/// also carry: the local ref is the one a checkout would use, and no fetch
/// can be needed for it.
#[test]
fn branch_origin_reads_the_local_ref_before_any_remote() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    git_ok(
        &clone,
        &["checkout", "-q", "-b", "feature-x", "origin/feature-x"],
    );
    let repo = git2::Repository::open(&clone).unwrap();

    assert_eq!(
        branch_origin(&repo, "feature-x").unwrap(),
        BranchOrigin::Local
    );
    assert_eq!(branch_origin(&repo, "main").unwrap(), BranchOrigin::Local);
}
/// A branch only a remote carries names the remote it would be fetched
/// from and the tracking ref it would be cut at.
#[test]
fn branch_origin_finds_a_branch_only_a_remote_carries() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let repo = git2::Repository::open(&clone).unwrap();

    assert_eq!(
        branch_origin(&repo, "feature-x").unwrap(),
        BranchOrigin::Remote {
            remote: "origin".to_string(),
            tracking_ref: "refs/remotes/origin/feature-x".to_string(),
        }
    );
}
/// Two remotes carrying one branch answer with the one a fetch comes from,
/// which is the same precedence the listing offers it under — `origin`
/// first, whatever the other remote sorts as.
#[test]
fn branch_origin_prefers_origin_over_another_remote() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let origin_path = dir.path().join("origin.git");
    git_ok(
        &clone,
        &["remote", "add", "fork", origin_path.to_str().unwrap()],
    );
    git_ok(&clone, &["fetch", "-q", "fork"]);
    let repo = git2::Repository::open(&clone).unwrap();

    assert_eq!(
        branch_origin(&repo, "feature-x").unwrap(),
        BranchOrigin::Remote {
            remote: "origin".to_string(),
            tracking_ref: "refs/remotes/origin/feature-x".to_string(),
        }
    );
}
/// A name no ref of any kind backs is absent — and so is `HEAD`, whose
/// `refs/remotes/origin/HEAD` is a pointer at a branch, not a branch. A
/// fetch of it would ask every remote for a ref none of them has.
#[test]
fn branch_origin_is_absent_for_an_unknown_name_and_for_a_symbolic_ref() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let repo = git2::Repository::open(&clone).unwrap();
    assert!(
        repo.find_reference("refs/remotes/origin/HEAD").is_ok(),
        "the clone has the symbolic ref this test is about"
    );

    assert_eq!(
        branch_origin(&repo, "nobody-cut-this").unwrap(),
        BranchOrigin::Absent
    );
    assert_eq!(branch_origin(&repo, "HEAD").unwrap(), BranchOrigin::Absent);
}
/// Only a missing ref means absent. A name git cannot even parse as a ref
/// is a caller's bug, and it is surfaced rather than answered.
#[test]
fn branch_origin_surfaces_an_error_that_is_not_a_missing_ref() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());
    let repo = git2::Repository::open(&clone).unwrap();

    assert!(branch_origin(&repo, "not a ref name").is_err());
}
