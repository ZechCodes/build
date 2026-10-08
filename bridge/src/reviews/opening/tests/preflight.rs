use super::*;

#[test]
fn initially_missing_base_leaves_the_checkout_intact_and_can_retry_after_restoration() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let mut request = request(home.path(), &source);
    request
        .request
        .base_branches
        .insert("directory-1".into(), "refs/heads/missing-base".into());
    let checkout = &request.workspace.directories[0].path;
    let repository = git2::Repository::open(checkout).unwrap();
    let original = repository.head().unwrap().target().unwrap();
    let error = open(&store, &request, &hooks()).unwrap_err();
    assert!(error.contains("base unavailable"), "{error}");
    let failed = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, ReviewOpeningState::Failed);
    assert!(failed.bindings.is_empty());
    assert!(store.load_tracker_task(&failed.task.id).unwrap().is_none());
    assert_eq!(
        repository.head().unwrap().name(),
        Some("refs/heads/build/work")
    );
    assert_eq!(repository.head().unwrap().target(), Some(original));
    assert_eq!(
        crate::isolation::branch_teardown(checkout).unwrap(),
        BranchTeardown::DeletesBranch
    );
    assert!(repository
        .references_glob("refs/heads/review/*")
        .unwrap()
        .next()
        .is_none());

    git_in(&source, &["branch", "missing-base", "main"]);
    let opened = open(&store, &request, &hooks()).unwrap();
    assert_eq!(opened.task.id, failed.task.id);
    assert_eq!(opened.review.snapshots.len(), 1);
}

#[test]
fn current_checkout_refuses_rebase_cherry_pick_and_revert_before_any_setup() {
    for operation in [
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        let (home, source) = init_repo();
        let store = Store::new(home.path().join("db")).unwrap();
        let request = request(home.path(), &source);
        let checkout = &request.workspace.directories[0].path;
        let repository = git2::Repository::open(checkout).unwrap();
        let original = repository.head().unwrap().target().unwrap();
        std::fs::write(checkout.join("README.md"), "staged\n").unwrap();
        git_in(checkout, &["add", "README.md"]);
        std::fs::write(checkout.join("README.md"), "unstaged\n").unwrap();
        let index_before = std::fs::read(repository.path().join("index")).unwrap();
        let marker = repository.path().join(operation);
        if operation.starts_with("rebase-") {
            std::fs::create_dir(&marker).unwrap();
            std::fs::write(marker.join("head-name"), "refs/heads/build/work\n").unwrap();
        } else {
            std::fs::write(&marker, format!("{original}\n")).unwrap();
        }

        let error = open(&store, &request, &hooks()).unwrap_err();
        assert!(
            error.contains("Git operation in progress"),
            "{operation}: {error}"
        );
        assert_eq!(
            repository.head().unwrap().name(),
            Some("refs/heads/build/work")
        );
        assert_eq!(
            std::fs::read(repository.path().join("index")).unwrap(),
            index_before
        );
        assert_eq!(
            std::fs::read(checkout.join("README.md")).unwrap(),
            b"unstaged\n"
        );
        assert!(marker.exists());
        assert!(repository
            .references_glob("refs/heads/review/*")
            .unwrap()
            .next()
            .is_none());
        let failed = store
            .load_review_opening(&request.project_path, &request.request_id)
            .unwrap()
            .unwrap();
        assert!(failed.bindings.is_empty());
        assert!(store.load_tracker_task(&failed.task.id).unwrap().is_none());
        assert_eq!(
            crate::isolation::branch_teardown(checkout).unwrap(),
            BranchTeardown::DeletesBranch
        );
    }
}
