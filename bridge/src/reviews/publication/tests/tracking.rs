use super::*;

fn tracking_ref(binding: &ReviewBranchBinding) -> String {
    format!(
        "refs/remotes/{}/{}",
        binding.remote_name,
        binding
            .dedicated_branch_ref
            .strip_prefix("refs/heads/")
            .unwrap()
    )
}

#[test]
fn already_received_initial_publication_repairs_the_owned_upstream_tracking_ref() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    import_commit(
        &binding.receiving_repository,
        &source,
        &binding.initial_head,
    )
    .unwrap();
    let receiver = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    receiver
        .reference(
            &binding.receiving_ref,
            git2::Oid::from_str(&binding.initial_head).unwrap(),
            false,
            "received before tracking",
        )
        .unwrap();
    publish_initial(&binding).unwrap();
    assert_eq!(oid(&source, &tracking_ref(&binding)), binding.initial_head);
    let repository = git2::Repository::open(&source).unwrap();
    assert_eq!(
        repository
            .find_branch("review/1-test", git2::BranchType::Local)
            .unwrap()
            .upstream()
            .unwrap()
            .get()
            .target()
            .unwrap()
            .to_string(),
        binding.initial_head
    );
    cleanup_initial(&binding).unwrap();
    assert!(repository.find_reference(&tracking_ref(&binding)).is_err());
}

#[test]
fn initial_publication_preserves_preexisting_unowned_tracking_refs() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    let repository = git2::Repository::open(&source).unwrap();
    let tracking = tracking_ref(&binding);
    repository
        .reference(
            &tracking,
            git2::Oid::from_str(&binding.initial_head).unwrap(),
            false,
            "user tracking ref",
        )
        .unwrap();
    assert!(publish_initial(&binding).is_err());
    assert_eq!(oid(&source, &tracking), binding.initial_head);
    cleanup_initial(&binding).unwrap();
    assert_eq!(oid(&source, &tracking), binding.initial_head);
}

#[test]
fn changed_or_symbolic_owned_tracking_refs_keep_their_cancellation_claim() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let tracking = tracking_ref(&binding);
    let repository = git2::Repository::open(&source).unwrap();
    git_in(&source, &["commit", "--allow-empty", "-m", "user change"]);
    let changed = repository.head().unwrap().target().unwrap();
    repository
        .reference(&tracking, changed, true, "user tracking change")
        .unwrap();
    assert!(publish_initial(&binding).is_err());
    assert!(cleanup_initial(&binding).is_err());
    assert_eq!(oid(&source, &tracking), changed.to_string());
    assert_eq!(
        received_head(&binding).unwrap(),
        Some(binding.initial_head.clone())
    );
    repository
        .reference_symbolic(&tracking, "refs/heads/main", true, "user symbolic tracking")
        .unwrap();
    assert!(publish_initial(&binding).is_err());
    assert!(cleanup_initial(&binding).is_err());
    assert_eq!(
        repository
            .find_reference(&tracking)
            .unwrap()
            .symbolic_target(),
        Some("refs/heads/main")
    );
    repository
        .reference(
            &tracking,
            git2::Oid::from_str(&binding.initial_head).unwrap(),
            true,
            "restore expected tracking",
        )
        .unwrap();
    cleanup_initial(&binding).unwrap();
    assert!(repository.find_reference(&tracking).is_err());
    assert_eq!(received_head(&binding).unwrap(), None);
}

#[test]
fn tracking_cleanup_preserves_refs_when_the_owned_alias_was_changed() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let repository = git2::Repository::open(&source).unwrap();
    repository
        .config()
        .unwrap()
        .set_str(
            &format!("remote.{}.url", binding.remote_name),
            "https://example.test/user.git",
        )
        .unwrap();
    assert!(cleanup_initial(&binding).is_err());
    assert_eq!(oid(&source, &tracking_ref(&binding)), binding.initial_head);
    assert_eq!(
        received_head(&binding).unwrap(),
        Some(binding.initial_head.clone())
    );
}

#[cfg(unix)]
fn interrupt_tracking_writer(binding_path: &Path, phase: &str) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reviews::publication::tests::tracking::interrupted_tracking_writes_recover_for_retry_and_cancellation", "--nocapture"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("BUILD_REVIEW_INTERRUPTED_TRACKING_BINDING", binding_path)
        .env("BUILD_REVIEW_INTERRUPTED_TRACKING_PHASE", phase)
        .status().unwrap();
    assert_eq!(status.code(), Some(27));
}

#[cfg(unix)]
#[test]
fn interrupted_tracking_writes_recover_for_retry_and_cancellation() {
    if let Some(path) = std::env::var_os("BUILD_REVIEW_INTERRUPTED_TRACKING_BINDING") {
        let binding: ReviewBranchBinding =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        if std::env::var("BUILD_REVIEW_INTERRUPTED_TRACKING_PHASE").unwrap() == "cleanup-locked" {
            cleanup_initial(&binding).unwrap();
        } else {
            publish_initial(&binding).unwrap();
        }
        panic!("tracking writer must exit at its requested interruption boundary");
    }
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    let binding_path = temporary.path().join("binding.json");
    fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();
    let repository = git2::Repository::open(&source).unwrap();
    let tracking = tracking_ref(&binding);
    let tracking_lock = repository.commondir().join(format!("{tracking}.lock"));
    let unrelated_lock = repository.commondir().join("refs/remotes/user-owned.lock");
    fs::create_dir_all(unrelated_lock.parent().unwrap()).unwrap();
    fs::write(&unrelated_lock, "user lock\n").unwrap();
    for phase in ["after-receiver", "tracking-locked"] {
        interrupt_tracking_writer(&binding_path, phase);
        assert_eq!(
            received_head(&binding).unwrap(),
            Some(binding.initial_head.clone())
        );
        assert!(repository.find_reference(&tracking).is_err());
        if phase == "tracking-locked" {
            assert!(tracking_lock.exists());
        }
        assert_eq!(publish_initial(&binding).unwrap(), binding.initial_head);
        assert_eq!(oid(&source, &tracking), binding.initial_head);
        assert!(!tracking_lock.exists());
        interrupt_tracking_writer(&binding_path, "cleanup-locked");
        assert!(tracking_lock.exists());
        cleanup_initial(&binding).unwrap();
        assert!(repository.find_reference(&tracking).is_err());
        assert!(!tracking_lock.exists());
        assert_eq!(received_head(&binding).unwrap(), None);
    }
    interrupt_tracking_writer(&binding_path, "tracking-locked");
    fs::remove_file(&tracking_lock).unwrap();
    fs::write(&tracking_lock, "replacement user lock\n").unwrap();
    assert!(publish_initial(&binding).is_err());
    assert!(cleanup_initial(&binding).is_err());
    assert_eq!(
        fs::read_to_string(tracking_lock).unwrap(),
        "replacement user lock\n"
    );
    assert_eq!(fs::read_to_string(unrelated_lock).unwrap(), "user lock\n");
}
