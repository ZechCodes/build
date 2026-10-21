use super::*;

fn committed_repository(directory: &Path) -> git2::Repository {
    let repository = git2::Repository::init(directory).unwrap();
    crate::git_fixture::configure_repo(directory);
    std::fs::write(directory.join("tracked.txt"), "original\n").unwrap();
    let mut index = repository.index().unwrap();
    index.add_path(Path::new("tracked.txt")).unwrap();
    index.write().unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("Test", "test@build.ing").unwrap();
    repository
        .commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap();
    drop(tree);
    repository
}

#[test]
fn current_source_rejects_changed_head_tree_tracked_and_untracked_state() {
    let directory = tempfile::tempdir().unwrap();
    let repository = committed_repository(directory.path());
    let commit = repository.head().unwrap().peel_to_commit().unwrap();
    let (mut provenance, _, _) = receipts();
    provenance.source_revision = commit.id().to_string();
    provenance.source_tree = commit.tree_id().to_string();
    validate_repository_source(&repository, &provenance).unwrap();
    let mut wrong_head = provenance.clone();
    wrong_head.source_revision = "0".repeat(40);
    assert!(validate_repository_source(&repository, &wrong_head).is_err());
    let mut wrong_tree = provenance.clone();
    wrong_tree.source_tree = "0".repeat(40);
    assert!(validate_repository_source(&repository, &wrong_tree).is_err());
    std::fs::write(directory.path().join("tracked.txt"), "changed\n").unwrap();
    assert!(validate_repository_source(&repository, &provenance).is_err());
    std::fs::write(directory.path().join("tracked.txt"), "original\n").unwrap();
    validate_repository_source(&repository, &provenance).unwrap();
    std::fs::write(directory.path().join("untracked.txt"), "unexpected\n").unwrap();
    assert!(validate_repository_source(&repository, &provenance).is_err());
    std::fs::remove_file(directory.path().join("untracked.txt")).unwrap();
    let signature = git2::Signature::now("Test", "test@build.ing").unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "new head",
            &commit.tree().unwrap(),
            &[&commit],
        )
        .unwrap();
    assert!(validate_repository_source(&repository, &provenance).is_err());
}
