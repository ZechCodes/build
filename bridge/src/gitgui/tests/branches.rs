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

#[test]
fn ref_list_contains_exact_local_branches_and_tags() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git_ok(dir.path(), &["branch", "topic/nested"]);
    git_ok(dir.path(), &["tag", "release-1"]);
    git_ok(dir.path(), &["tag", "-a", "release-2", "-m", "annotated"]);

    let listing = ref_list(dir.path()).unwrap();
    assert_eq!(
        listing.current,
        CurrentRef::Branch {
            name: "main".to_string(),
            full_ref: "refs/heads/main".to_string(),
        }
    );
    assert_eq!(listing.refs.len(), 4);
    assert_eq!(listing.refs[0].name, "main");
    assert!(listing.refs[0].current);
    assert_eq!(listing.refs[0].kind, RefKind::Branch);
    assert_eq!(listing.refs[0].full_ref, "refs/heads/main");
    assert_eq!(listing.refs[1].full_ref, "refs/heads/topic/nested");
    assert_eq!(listing.refs[2].full_ref, "refs/tags/release-1");
    assert_eq!(listing.refs[3].full_ref, "refs/tags/release-2");
    assert!(listing.refs[1..].iter().all(|row| !row.current));
}

#[test]
fn ref_listing_json_keeps_ref_kind_and_full_name_explicit() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git_ok(dir.path(), &["tag", "release"]);

    let payload = ref_list(dir.path()).unwrap().into_json();

    assert_eq!(payload["current"]["kind"], "branch");
    assert_eq!(payload["current"]["name"], "main");
    assert_eq!(payload["current"]["full_ref"], "refs/heads/main");
    assert_eq!(payload["refs"][0]["kind"], "branch");
    assert_eq!(payload["refs"][0]["current"], true);
    assert_eq!(payload["refs"][1]["kind"], "tag");
    assert_eq!(payload["refs"][1]["name"], "release");
    assert_eq!(payload["refs"][1]["full_ref"], "refs/tags/release");
}

#[test]
fn ref_list_excludes_remote_tracking_and_symbolic_refs() {
    let dir = tempfile::tempdir().unwrap();
    let clone = clone_of_an_origin_carrying_feature_x(dir.path());

    let listing = ref_list(&clone).unwrap();

    assert_eq!(listing.refs.len(), 1, "{:#?}", listing.refs);
    assert_eq!(listing.refs[0].full_ref, "refs/heads/main");
}

#[test]
fn ref_list_includes_an_unborn_current_branch() {
    let dir = tempfile::tempdir().unwrap();
    git_ok(dir.path(), &["init", "-q", "-b", "main"]);

    let listing = ref_list(dir.path()).unwrap();

    assert_eq!(
        listing.current,
        CurrentRef::Branch {
            name: "main".to_string(),
            full_ref: "refs/heads/main".to_string(),
        }
    );
    assert_eq!(listing.refs.len(), 1);
    assert_eq!(listing.refs[0].full_ref, "refs/heads/main");
    assert!(listing.refs[0].current);
}

#[test]
fn checkout_ref_attaches_branches_and_detaches_tags() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git_ok(dir.path(), &["branch", "topic"]);
    git_ok(dir.path(), &["tag", "release-1"]);

    checkout_ref(dir.path(), "refs/heads/topic").unwrap();
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/topic"));
    drop(repo);

    checkout_ref(dir.path(), "refs/tags/release-1").unwrap();
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert!(repo.head_detached().unwrap());
    let commit = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    drop(repo);
    let listing = ref_list(dir.path()).unwrap();
    assert_eq!(listing.current, CurrentRef::Detached { commit });
    assert!(listing.refs.iter().all(|row| !row.current));
}

#[test]
fn checkout_ref_carries_compatible_local_changes() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git_ok(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "feature.txt", "feature\n");
    git_ok(dir.path(), &["add", "feature.txt"]);
    git_ok(dir.path(), &["commit", "-q", "-m", "feature"]);
    git_ok(dir.path(), &["checkout", "-q", "main"]);
    write(dir.path(), "f.txt", "local edit\n");

    checkout_ref(dir.path(), "refs/heads/feature").unwrap();

    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/feature"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "local edit\n"
    );
}

#[test]
fn checkout_ref_refuses_to_overwrite_local_changes_without_mutating_them() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    git_ok(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "f.txt", "feature\n");
    git_ok(dir.path(), &["commit", "-q", "-am", "feature"]);
    git_ok(dir.path(), &["checkout", "-q", "main"]);
    write(dir.path(), "f.txt", "local edit\n");

    let error = checkout_ref(dir.path(), "refs/heads/feature").unwrap_err();

    assert!(error.contains("local changes"), "{error}");
    assert!(error.contains("f.txt"), "{error}");
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/main"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "local edit\n"
    );
}

#[test]
fn checkout_ref_requires_an_existing_full_branch_or_tag_ref() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());

    for invalid in [
        "main",
        "refs/remotes/origin/main",
        "refs/heads/missing",
        "refs/tags/missing",
        "refs/heads/-",
        "refs/heads/--force",
        "refs/tags/bad name",
    ] {
        assert!(
            checkout_ref(dir.path(), invalid).is_err(),
            "accepted {invalid}"
        );
    }
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/main"));
}

#[test]
fn checkout_ref_does_not_overwrite_an_ignored_local_file() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    write(dir.path(), ".gitignore", "ignored.txt\n");
    git_ok(dir.path(), &["add", ".gitignore"]);
    git_ok(dir.path(), &["commit", "-q", "-m", "ignore local file"]);
    git_ok(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "ignored.txt", "feature version\n");
    git_ok(dir.path(), &["add", "-f", "ignored.txt"]);
    git_ok(dir.path(), &["commit", "-q", "-m", "track ignored file"]);
    git_ok(dir.path(), &["checkout", "-q", "main"]);
    write(dir.path(), "ignored.txt", "local ignored version\n");

    let error = checkout_ref(dir.path(), "refs/heads/feature").unwrap_err();

    assert!(error.contains("overwritten"), "{error}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ignored.txt")).unwrap(),
        "local ignored version\n"
    );
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/main"));
}

#[test]
fn checkout_ref_refuses_a_tag_that_does_not_resolve_to_a_commit() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let object = git_run(dir.path(), &["hash-object", "-w", "f.txt"]);
    let oid = String::from_utf8_lossy(&object.stdout).trim().to_string();
    git_ok(dir.path(), &["update-ref", "refs/tags/blob", &oid]);

    let error = checkout_ref(dir.path(), "refs/tags/blob").unwrap_err();

    assert!(error.contains("does not point to a commit"), "{error}");
    let repo = git2::Repository::open(dir.path()).unwrap();
    assert_eq!(repo.head().unwrap().name(), Some("refs/heads/main"));
}
