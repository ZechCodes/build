use super::*;
use crate::git_fixture::{git_command, git_in, init_repo};
use crate::reviews::model::{ReviewPreparationState, ReviewPublicationState};
use crate::reviews::receivers::{ensure_receiver, plan_receiver};
use crate::workspace::{DirectoryStatus, WorkspaceDirectory, WorkspaceStatus};
use std::path::Path;

fn oid(repository: &Path, reference: &str) -> String {
    String::from_utf8(
        git_command(repository, &["rev-parse", reference])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .into()
}

fn binding(source: &Path, receiver_root: &Path) -> ReviewBranchBinding {
    let receiver = plan_receiver(source, receiver_root).unwrap();
    ensure_receiver(&receiver).unwrap();
    let dedicated = "refs/heads/review/1-test";
    git_in(
        source,
        &[
            "checkout",
            "-b",
            dedicated.strip_prefix("refs/heads/").unwrap(),
        ],
    );
    let mut binding = ReviewBranchBinding {
        directory_id: "directory-1".into(),
        source_id: "source-1".into(),
        repository_id: receiver.repository_id,
        working_repository: source.into(),
        source_repository: source.into(),
        initial_head: oid(source, "HEAD"),
        original_branch_ref: Some("refs/heads/main".into()),
        dedicated_branch_ref: dedicated.into(),
        base_branch_ref: "refs/heads/main".into(),
        receiving_repository: receiver.path,
        receiving_ref: dedicated.into(),
        remote_name: String::new(),
        last_received_head: None,
        preparation: ReviewPreparationState::BranchCreated,
        publication: ReviewPublicationState::Pending,
        recovery: None,
    };
    binding.remote_name = choose_remote_name(&binding).unwrap();
    binding
}

#[test]
fn publication_preserves_origin_and_push_defaults_and_plain_push_works() {
    let (temporary, source) = init_repo();
    git_in(
        &source,
        &["remote", "add", "origin", "https://example.test/source.git"],
    );
    git_in(&source, &["config", "remote.pushDefault", "origin"]);
    git_in(&source, &["config", "push.default", "simple"]);
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    assert_eq!(publish_initial(&binding).unwrap(), binding.initial_head);
    assert_eq!(publish_initial(&binding).unwrap(), binding.initial_head);
    std::fs::write(source.join("untracked.txt"), "untracked").unwrap();
    std::fs::write(source.join("README.md"), "changed").unwrap();
    git_in(&source, &["add", "README.md"]);
    git_in(&source, &["commit", "-m", "review commit"]);
    git_in(&source, &["push"]);
    assert_eq!(
        oid(&binding.receiving_repository, &binding.receiving_ref),
        oid(&source, "HEAD")
    );
    assert!(source.join("untracked.txt").exists());
    assert!(publish_initial(&binding)
        .unwrap_err()
        .to_string()
        .contains("changed"));
    assert_eq!(
        crate::git_process::run_git(&source, &["config", "remote.pushDefault"])
            .unwrap()
            .trim(),
        "origin"
    );
    assert_eq!(
        crate::git_process::run_git(&source, &["remote", "get-url", "origin"])
            .unwrap()
            .trim(),
        "https://example.test/source.git"
    );
    cleanup_remote(&binding).unwrap();
    let config = git2::Repository::open(&source).unwrap().config().unwrap();
    assert_eq!(
        config.get_string("remote.origin.url").unwrap(),
        "https://example.test/source.git"
    );
    assert_eq!(config.get_string("remote.pushDefault").unwrap(), "origin");
    assert_eq!(config.get_string("push.default").unwrap(), "simple");
    for (key, _) in configuration(&binding).unwrap() {
        assert!(config_values(&config, &key).unwrap().is_empty());
    }
}

#[test]
fn an_existing_build_review_remote_is_preserved() {
    let (temporary, source) = init_repo();
    git_in(
        &source,
        &[
            "remote",
            "add",
            "build-review",
            "https://example.test/users-review.git",
        ],
    );
    let binding = binding(&source, &temporary.path().join("receivers"));
    assert_ne!(binding.remote_name, "build-review");
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    assert_eq!(
        crate::git_process::run_git(&source, &["remote", "get-url", "build-review"])
            .unwrap()
            .trim(),
        "https://example.test/users-review.git"
    );
}

#[test]
fn cleanup_keeps_configuration_the_user_changed() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    git_in(
        &source,
        &[
            "config",
            &format!(
                "branch.{}.pushRemote",
                binding
                    .dedicated_branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap()
            ),
            "origin",
        ],
    );
    assert!(cleanup_remote(&binding).unwrap_err().contains("changed"));
    assert_eq!(
        crate::git_process::run_git(
            &source,
            &[
                "config",
                &format!(
                    "branch.{}.pushRemote",
                    binding
                        .dedicated_branch_ref
                        .strip_prefix("refs/heads/")
                        .unwrap()
                )
            ]
        )
        .unwrap()
        .trim(),
        "origin"
    );
}

#[test]
fn initial_cleanup_refuses_an_advanced_receiving_branch() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    git_in(&source, &["commit", "--allow-empty", "-m", "later"]);
    git_in(&source, &["push"]);
    assert!(cleanup_initial(&binding).unwrap_err().contains("changed"));
    assert_eq!(
        oid(&binding.receiving_repository, &binding.receiving_ref),
        oid(&source, "HEAD")
    );
}

#[test]
fn preflight_rejects_unrelated_source_base_before_any_publication() {
    let (temporary, source) = init_repo();
    let mut binding = binding(&source, &temporary.path().join("receivers"));
    git_in(&source, &["checkout", "--orphan", "unrelated"]);
    git_in(&source, &["commit", "-m", "unrelated"]);
    git_in(&source, &["checkout", "review/1-test"]);
    binding.base_branch_ref = "refs/heads/unrelated".into();
    assert!(validate_bases(&[binding.clone()])
        .unwrap_err()
        .contains("unrelated"));
    assert!(!git_command(
        &binding.receiving_repository,
        &["show-ref", "--verify", &binding.receiving_ref]
    )
    .output()
    .unwrap()
    .status
    .success());
}

fn workspace(binding: &ReviewBranchBinding) -> Workspace {
    Workspace {
        id: "workspace-1".into(),
        project_id: "project-1".into(),
        name: "review".into(),
        root: binding.working_repository.parent().unwrap().into(),
        status: WorkspaceStatus::Ready,
        archived_at: None,
        directories: vec![WorkspaceDirectory {
            id: binding.directory_id.clone(),
            source_id: binding.source_id.clone(),
            name: "repo".into(),
            path: binding.working_repository.clone(),
            source_path: binding.source_repository.clone(),
            is_git: true,
            branch: Some("review/1-test".into()),
            effective_isolation: None,
            finished_head: None,
            status: DirectoryStatus::Ready,
            base_branch: "main".into(),
            error: None,
        }],
        isolation: Default::default(),
        managed: false,
        created_by_agent: false,
        locked: false,
    }
}

fn capture(binding: &ReviewBranchBinding, snapshot_id: &str) -> ReviewSnapshot {
    capture_snapshot(
        "task-1",
        snapshot_id,
        &workspace(binding),
        std::slice::from_ref(binding),
        &[ReviewMembership {
            directory_id: binding.directory_id.clone(),
            source_id: binding.source_id.clone(),
            kind: ReviewMembershipKind::Git,
            reason: None,
        }],
        &Actor::User,
    )
    .unwrap()
}

#[test]
fn snapshot_uses_merge_base_and_survives_removing_the_source() {
    let (temporary, source) = init_repo();
    let initial = oid(&source, "HEAD");
    git_in(&source, &["checkout", "-b", "feature"]);
    std::fs::write(source.join("feature.txt"), "published\n").unwrap();
    git_in(&source, &["add", "feature.txt"]);
    git_in(&source, &["commit", "-m", "feature"]);
    let binding = binding(&source, &temporary.path().join("receivers"));
    git_in(&source, &["checkout", "main"]);
    std::fs::write(source.join("source-only.txt"), "new base\n").unwrap();
    git_in(&source, &["add", "source-only.txt"]);
    git_in(&source, &["commit", "-m", "source base"]);
    git_in(&source, &["checkout", "review/1-test"]);
    std::fs::write(source.join("dirty.txt"), "uncommitted\n").unwrap();
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let snapshot = capture(&binding, "opening-task-1");
    assert_eq!(snapshot.directories[0].base.as_ref().unwrap().oid, initial);
    assert_eq!(snapshot.directories[0].uncommitted_files, Some(1));
    let replay = capture(&binding, "opening-task-1");
    assert_eq!(snapshot.directories, replay.directories);
    std::fs::remove_dir_all(&source).unwrap();
    git_in(&binding.receiving_repository, &["gc", "--prune=now"]);
    let request = crate::reviews::read::ReviewReadRequest {
        mode: crate::reviews::read::ReviewReadMode::Blob,
        path: Some("feature.txt".into()),
        paths: vec![],
        range: None,
        patch: true,
    };
    let crate::reviews::read::ReviewReadResult::Blob(blob) =
        crate::reviews::read::read(&snapshot.directories[0], &request).unwrap()
    else {
        panic!("blob expected")
    };
    assert_eq!(blob.content_b64, crate::encoding::b64encode(b"published\n"));
    crate::reviews::capture::cleanup_pins("task-1", &snapshot).unwrap();
    let prefix =
        crate::reviews::capture::pin_prefix("task-1", &snapshot.id, &binding.directory_id).unwrap();
    assert!(!git_command(
        &binding.receiving_repository,
        &["show-ref", "--verify", &format!("{prefix}/target")]
    )
    .output()
    .unwrap()
    .status
    .success());
    assert!(!snapshot_marker(&binding, "task-1", &snapshot.id).exists());
}

#[test]
fn restart_keeps_the_source_base_recorded_before_creating_pins() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let planned = plan_snapshot_pins(&binding, "task-1", "opening-task-1").unwrap();
    git_in(&source, &["checkout", "main"]);
    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "new source base"],
    );
    git_in(&source, &["checkout", "review/1-test"]);
    capture(&binding, "opening-task-1");
    let prefix =
        crate::reviews::capture::pin_prefix("task-1", "opening-task-1", &binding.directory_id)
            .unwrap();
    assert_eq!(
        oid(&binding.receiving_repository, &format!("{prefix}/target")),
        planned.source_base
    );
    cleanup_opening_pins("task-1", "opening-task-1", std::slice::from_ref(&binding)).unwrap();
    cleanup_opening_pins("task-1", "opening-task-1", std::slice::from_ref(&binding)).unwrap();
    assert!(!git_command(
        &binding.receiving_repository,
        &["show-ref", "--verify", &format!("{prefix}/head")]
    )
    .output()
    .unwrap()
    .status
    .success());
}

#[test]
fn shared_configuration_aliases_are_reserved_during_planning() {
    let (temporary, source) = init_repo();
    let first = binding(&source, &temporary.path().join("receivers"));
    let mut second = first.clone();
    second.dedicated_branch_ref = "refs/heads/review/1-other".into();
    second.receiving_ref = second.dedicated_branch_ref.clone();
    let reserved = BTreeSet::from([first.remote_name.clone()]);
    second.remote_name = choose_remote_name_avoiding(&second, &reserved).unwrap();
    assert_ne!(first.remote_name, second.remote_name);
    configure_remote(&first).unwrap();
    configure_remote(&second).unwrap();
}

#[test]
fn metadata_publication_locks_the_received_head_and_snapshot_pins() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    capture(&binding, "opening-task-1");
    validate_snapshot_pins("task-1", "opening-task-1", std::slice::from_ref(&binding)).unwrap();
    let value = with_initial_receivers_locked(
        std::slice::from_ref(&binding),
        "task-1",
        "opening-task-1",
        || {
            let reference = git_command(
                &binding.receiving_repository,
                &["update-ref", &binding.receiving_ref, &binding.initial_head],
            )
            .output()
            .unwrap();
            assert!(!reference.status.success());
            let prefix = crate::reviews::capture::pin_prefix(
                "task-1",
                "opening-task-1",
                &binding.directory_id,
            )
            .unwrap();
            let pin = git_command(
                &binding.receiving_repository,
                &[
                    "update-ref",
                    &format!("{prefix}/head"),
                    &binding.initial_head,
                ],
            )
            .output()
            .unwrap();
            assert!(!pin.status.success());
            Ok(42)
        },
    )
    .unwrap();
    assert_eq!(value, 42);
    git_in(&source, &["commit", "--allow-empty", "-m", "later"]);
    git_in(&source, &["push"]);
    assert!(
        validate_snapshot_pins("task-1", "opening-task-1", std::slice::from_ref(&binding))
            .unwrap_err()
            .contains("changed")
    );
    assert!(with_initial_receivers_locked::<()>(
        std::slice::from_ref(&binding),
        "task-1",
        "opening-task-1",
        || panic!("metadata must not publish a changed received head")
    )
    .is_err());
}

#[test]
fn imports_refuse_rewritten_local_source_urls() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    let other = temporary.path().join("other.git");
    git_in(
        temporary.path(),
        &["init", "--bare", other.to_str().unwrap()],
    );
    git_in(
        &binding.receiving_repository,
        &[
            "config",
            &format!("url.{}.insteadOf", other.to_str().unwrap()),
            source.to_str().unwrap(),
        ],
    );
    let error = validate_bases(std::slice::from_ref(&binding)).unwrap_err();
    assert!(error.contains("local review source"), "{error}");
}

#[test]
fn plain_push_uses_only_this_review_branch_with_any_push_default() {
    let (temporary, source) = init_repo();
    let first = binding(&source, &temporary.path().join("receivers"));
    let mut second = first.clone();
    second.dedicated_branch_ref = "refs/heads/review/2-test".into();
    second.receiving_ref = second.dedicated_branch_ref.clone();
    second.remote_name =
        choose_remote_name_avoiding(&second, &BTreeSet::from([first.remote_name.clone()])).unwrap();
    git_in(&source, &["branch", "review/2-test"]);
    configure_remote(&first).unwrap();
    configure_remote(&second).unwrap();
    publish_initial(&first).unwrap();
    publish_initial(&second).unwrap();
    git_in(&source, &["checkout", "review/2-test"]);
    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "second pending"],
    );
    git_in(&source, &["checkout", "review/1-test"]);
    for default in ["matching", "nothing"] {
        git_in(&source, &["config", "push.default", default]);
        git_in(&source, &["commit", "--allow-empty", "-m", default]);
        git_in(&source, &["push"]);
        assert_eq!(
            oid(&first.receiving_repository, &first.receiving_ref),
            oid(&source, "HEAD")
        );
        assert_eq!(
            oid(&second.receiving_repository, &second.receiving_ref),
            second.initial_head
        );
    }
}

#[test]
fn cancelling_planned_bindings_without_a_receiver_is_a_noop() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    std::fs::remove_dir_all(&binding.receiving_repository).unwrap();
    cleanup_initial(&binding).unwrap();
    cleanup_opening_pins("task-1", "opening-task-1", &[binding]).unwrap();
}

#[test]
fn local_configuration_is_locked_across_validation_and_commit() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    with_local_config_locked(&source, |_, local| {
        let attempted = git_command(&source, &["config", "remote.pushDefault", "user-remote"])
            .output()
            .unwrap();
        assert!(!attempted.status.success());
        for (key, value) in configuration(&binding)? {
            local
                .set_str(&key, &value)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    assert!(!source.join(".git/config.lock").exists());
    git_in(
        &source,
        &[
            "config",
            &format!("remote.{}.url", binding.remote_name),
            "https://example.test/changed.git",
        ],
    );
    assert!(configure_remote(&binding).unwrap_err().contains("changed"));
    assert!(cleanup_remote(&binding).unwrap_err().contains("changed"));
    assert_eq!(
        crate::git_process::run_git(
            &source,
            &["config", &format!("remote.{}.url", binding.remote_name)]
        )
        .unwrap()
        .trim(),
        "https://example.test/changed.git"
    );
}

#[cfg(unix)]
fn interrupt_publication_writer(binding_file: &Path) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reviews::publication::tests::interrupted_metadata_publication_recovers_owned_ref_locks", "--nocapture"])
        .env("BUILD_REVIEW_PUBLICATION_BINDING", binding_file)
        .status().unwrap();
    assert_eq!(status.code(), Some(24));
}

#[cfg(unix)]
#[test]
fn interrupted_metadata_publication_recovers_owned_ref_locks() {
    if let Some(path) = std::env::var_os("BUILD_REVIEW_PUBLICATION_BINDING") {
        let binding: ReviewBranchBinding =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        with_initial_receivers_locked::<()>(
            std::slice::from_ref(&binding),
            "task-1",
            "opening-task-1",
            || {
                std::process::exit(24);
            },
        )
        .unwrap();
        panic!("interrupted writer must exit inside the publication transaction");
    }
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    capture(&binding, "opening-task-1");
    let binding_file = temporary.path().join("binding.json");
    std::fs::write(&binding_file, serde_json::to_vec(&binding).unwrap()).unwrap();
    interrupt_publication_writer(&binding_file);
    let receiving_lock = binding
        .receiving_repository
        .join(format!("{}.lock", binding.receiving_ref));
    assert!(receiving_lock.exists());
    let result = with_initial_receivers_locked(
        std::slice::from_ref(&binding),
        "task-1",
        "opening-task-1",
        || Ok(123),
    )
    .unwrap();
    assert_eq!(result, 123);
    assert!(!receiving_lock.exists());
    interrupt_publication_writer(&binding_file);
    std::fs::remove_file(&receiving_lock).unwrap();
    std::fs::write(&receiving_lock, "user lock\n").unwrap();
    let error = with_initial_receivers_locked::<()>(
        std::slice::from_ref(&binding),
        "task-1",
        "opening-task-1",
        || panic!("replaced user lock must block publication"),
    )
    .unwrap_err();
    assert!(error.contains("prove it owns"));
    assert_eq!(
        std::fs::read_to_string(&receiving_lock).unwrap(),
        "user lock\n"
    );
}

#[cfg(unix)]
fn interrupt_pin_writer(receiver: &Path, expected: &str, operation: &str) {
    let repository = git2::Repository::open_bare(receiver).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reviews::publication::tests::interrupted_pin_mutations_recover_without_touching_changed_refs_or_user_locks", "--nocapture"])
        .env("BUILD_REVIEW_INTERRUPTED_PIN_DIR", repository.path())
        .env("BUILD_REVIEW_INTERRUPTED_PIN_OPERATION", operation)
        .env("BUILD_REVIEW_INTERRUPTED_PIN_OID", expected)
        .status().unwrap();
    assert_eq!(status.code(), Some(25));
}

#[cfg(unix)]
#[test]
fn interrupted_pin_mutations_recover_without_touching_changed_refs_or_user_locks() {
    let reference = "refs/build/reviews/interrupted/head";
    if let Some(receiver) = std::env::var_os("BUILD_REVIEW_INTERRUPTED_PIN_DIR") {
        let repository = git2::Repository::open_bare(receiver).unwrap();
        let expected = std::env::var("BUILD_REVIEW_INTERRUPTED_PIN_OID").unwrap();
        if std::env::var("BUILD_REVIEW_INTERRUPTED_PIN_OPERATION").unwrap() == "create" {
            create_expected_pin(&repository, reference, &expected).unwrap();
        } else {
            remove_expected_ref(&repository, reference, &expected).unwrap();
        }
        panic!("pin writer must exit while its Git lock is held");
    }
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let repository = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let lock = repository.path().join(format!("{reference}.lock"));

    interrupt_pin_writer(repository.path(), &binding.initial_head, "create");
    assert!(lock.exists());
    create_expected_pin(&repository, reference, &binding.initial_head).unwrap();
    assert_eq!(oid(repository.path(), reference), binding.initial_head);
    assert!(!lock.exists());

    interrupt_pin_writer(repository.path(), &binding.initial_head, "remove");
    assert!(lock.exists());
    remove_expected_ref(&repository, reference, &binding.initial_head).unwrap();
    assert!(repository.find_reference(reference).is_err());
    assert!(!lock.exists());

    create_expected_pin(&repository, reference, &binding.initial_head).unwrap();
    interrupt_pin_writer(repository.path(), &binding.initial_head, "remove");
    fs::remove_file(&lock).unwrap();
    fs::write(&lock, "user lock\n").unwrap();
    assert!(remove_expected_ref(&repository, reference, &binding.initial_head).is_err());
    assert_eq!(fs::read_to_string(&lock).unwrap(), "user lock\n");
    assert_eq!(oid(repository.path(), reference), binding.initial_head);
    fs::remove_file(&lock).unwrap();

    // Retry after a dead writer must still inspect the current OID under its
    // new lock, rather than deleting a pin that the user advanced meanwhile.
    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "external change"],
    );
    let changed = oid(&source, "HEAD");
    import_commit(repository.path(), &source, &changed).unwrap();
    repository
        .reference(
            reference,
            git2::Oid::from_str(&changed).unwrap(),
            true,
            "external change",
        )
        .unwrap();
    assert!(
        remove_expected_ref(&repository, reference, &binding.initial_head)
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(oid(repository.path(), reference), changed);
}

#[test]
fn pin_cleanup_preserves_other_packed_refs_and_refuses_changed_shadow_entries() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let repository = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let reference = "refs/build/reviews/packed/head";
    let unrelated = "refs/heads/user-branch";
    create_expected_pin(&repository, reference, &binding.initial_head).unwrap();
    repository
        .reference(
            unrelated,
            git2::Oid::from_str(&binding.initial_head).unwrap(),
            false,
            "user ref",
        )
        .unwrap();
    git_in(repository.path(), &["pack-refs", "--all"]);
    let packed_path = repository.path().join("packed-refs");
    let original = fs::read_to_string(&packed_path).unwrap();
    remove_expected_ref(&repository, reference, &binding.initial_head).unwrap();
    assert!(repository.find_reference(reference).is_err());
    assert_eq!(oid(repository.path(), unrelated), binding.initial_head);
    assert_eq!(
        fs::read_to_string(&packed_path).unwrap(),
        original
            .lines()
            .filter(|line| !line.ends_with(reference))
            .map(|line| format!("{line}\n"))
            .collect::<String>()
    );

    git_in(
        &source,
        &["commit", "--allow-empty", "-m", "external packed change"],
    );
    let changed = oid(&source, "HEAD");
    import_commit(repository.path(), &source, &changed).unwrap();
    create_expected_pin(&repository, reference, &binding.initial_head).unwrap();
    // A changed packed entry may be hidden by the original loose pin. Neither
    // entry may be removed merely because Git currently resolves the loose one.
    let mut packed = fs::read(&packed_path).unwrap();
    packed.extend_from_slice(format!("{changed} {reference}\n").as_bytes());
    fs::write(&packed_path, &packed).unwrap();
    let error = remove_expected_ref(&repository, reference, &binding.initial_head).unwrap_err();
    assert!(error.contains("packed ref changed"));
    assert_eq!(fs::read(&packed_path).unwrap(), packed);
    assert_eq!(oid(repository.path(), reference), binding.initial_head);
}

#[test]
fn pin_cleanup_refuses_a_replaced_held_lock_before_deleting_the_ref() {
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    publish_initial(&binding).unwrap();
    let repository = git2::Repository::open_bare(&binding.receiving_repository).unwrap();
    let reference = "refs/build/reviews/replaced/head";
    create_expected_pin(&repository, reference, &binding.initial_head).unwrap();
    for relative in [format!("{reference}.lock"), "packed-refs.lock".into()] {
        let lock = repository.path().join(relative);
        let error = super::super::receivers::refs::remove_expected_reference(
            &repository,
            reference,
            git2::Oid::from_str(&binding.initial_head).unwrap(),
            || {
                fs::remove_file(&lock).unwrap();
                fs::write(&lock, "user lock\n").unwrap();
            },
        )
        .unwrap_err();
        assert!(error.contains("replaced"));
        assert_eq!(oid(repository.path(), reference), binding.initial_head);
        assert_eq!(fs::read_to_string(&lock).unwrap(), "user lock\n");
        fs::remove_file(lock).unwrap();
    }
}
