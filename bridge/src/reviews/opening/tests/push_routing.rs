use super::*;

fn refs(repository: &git2::Repository) -> BTreeMap<String, git2::Oid> {
    repository
        .references()
        .unwrap()
        .map(|reference| {
            let reference = reference.unwrap();
            (
                reference.name().unwrap().into(),
                reference.target().unwrap(),
            )
        })
        .collect()
}

fn assert_ui_push_uses_review_receiver(with_origin: bool) {
    let (home, source) = init_repo();
    let origin = if with_origin {
        let path = home.path().join("origin.git");
        let repository = git2::Repository::init_bare(&path).unwrap();
        git_in(
            &source,
            &["remote", "add", "origin", path.to_str().unwrap()],
        );
        git_in(&source, &["push", "origin", "main"]);
        Some(repository)
    } else {
        None
    };
    let origin_before = origin.as_ref().map(refs);
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let opened = open(&store, &request, &hooks()).unwrap();
    let binding = &opened.review.bindings[0];
    let checkout = &request.workspace.directories[0].path;
    let initial_status = crate::gitgui::status_payload(checkout).unwrap();
    let initial_summary = crate::gitgui::work_summary(checkout).unwrap();

    std::fs::write(checkout.join("README.md"), "review change\n").unwrap();
    git_in(checkout, &["add", "README.md"]);
    git_in(checkout, &["commit", "-m", "Review change"]);
    let working = git2::Repository::open(checkout).unwrap();
    let changed = working.head().unwrap().target().unwrap();
    assert_ne!(changed.to_string(), binding.initial_head);
    crate::gitgui::push(checkout, false).unwrap();

    assert_eq!(origin.as_ref().map(refs), origin_before);
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    assert_eq!(
        receiver.refname_to_id(&binding.receiving_ref).unwrap(),
        changed
    );
    let short_branch = binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .unwrap();
    assert_eq!(
        working
            .config()
            .unwrap()
            .get_string(&format!("branch.{short_branch}.remote"))
            .unwrap(),
        binding.remote_name
    );
    assert_eq!(
        initial_status["upstream"],
        format!("{}/{short_branch}", binding.remote_name)
    );
    assert_eq!(initial_status["ahead"], 0);
    assert_eq!(initial_status["behind"], 0);
    assert_eq!(initial_summary.pushes, 0);
    assert_eq!(crate::gitgui::work_summary(checkout).unwrap().pushes, 0);
}

#[test]
fn ui_push_after_opening_advances_the_review_receiver_and_preserves_origin() {
    assert_ui_push_uses_review_receiver(true);
}

#[test]
fn ui_push_after_opening_works_without_origin() {
    assert_ui_push_uses_review_receiver(false);
}

#[test]
fn cancel_preserves_all_setup_when_owned_tracking_ref_has_advanced() {
    let (home, source) = init_repo();
    let store = Store::new(home.path().join("db")).unwrap();
    let request = request(home.path(), &source);
    let mut checks = hooks();
    checks.fail_at = Some(OpeningStep::SnapshotPinned);
    assert!(open(&store, &request, &checks).is_err());
    let opening = store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap();
    let binding = &opening.bindings[0];
    let working = git2::Repository::open(&binding.working_repository).unwrap();
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let saved_refs = refs(&receiver);
    let config = std::fs::read(working.commondir().join("config")).unwrap();
    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "Another source commit"],
    );
    let advanced = git2::Repository::open(&source)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    let branch = binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .unwrap();
    let tracking = format!("refs/remotes/{}/{branch}", binding.remote_name);
    working
        .reference(&tracking, advanced, true, "external tracking change")
        .unwrap();

    assert!(cancel(&store, &request, &hooks()).is_err());
    assert_eq!(refs(&receiver), saved_refs);
    assert_eq!(working.refname_to_id(&tracking).unwrap(), advanced);
    assert_eq!(
        std::fs::read(working.commondir().join("config")).unwrap(),
        config
    );
    assert_eq!(
        working.head().unwrap().name(),
        Some(binding.dedicated_branch_ref.as_str())
    );
    assert_eq!(
        crate::isolation::branch_teardown(&binding.working_repository).unwrap(),
        BranchTeardown::KeepsBranch
    );
    assert!(store
        .load_review_opening(&request.project_path, &request.request_id)
        .unwrap()
        .unwrap()
        .state
        .is_claiming_workspace());
}
