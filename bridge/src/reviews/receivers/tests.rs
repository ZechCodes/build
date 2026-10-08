use super::*;
use crate::git_fixture::{git_in, init_repo};

#[test]
fn source_worktrees_share_identity_and_copies_have_their_own() {
    let (temporary, source) = init_repo();
    let worktree = temporary.path().join("worktree");
    git_in(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            worktree.to_str().unwrap(),
        ],
    );
    let copy = temporary.path().join("copy");
    git_in(
        temporary.path(),
        &["clone", source.to_str().unwrap(), copy.to_str().unwrap()],
    );
    let id = repository_id(&source).unwrap();
    assert_eq!(id.len(), 64);
    assert_eq!(id, repository_id(&worktree).unwrap());
    assert_ne!(id, repository_id(&copy).unwrap());
}

#[test]
fn receiver_is_owned_bare_and_reusable_without_alternates() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    ensure_receiver(&receiver).unwrap();
    assert!(git2::Repository::open_bare(&receiver.path)
        .unwrap()
        .is_bare());
    assert!(!receiver.path.join("objects/info/alternates").exists());
    assert!(receiver.path.join(".build-review-receiver.json").is_file());
    std::fs::write(
        receiver.path.join("objects/info/alternates"),
        source.join(".git/objects").to_str().unwrap(),
    )
    .unwrap();
    assert!(ensure_receiver(&receiver)
        .unwrap_err()
        .contains("alternates"));
}

#[test]
fn existing_unmarked_repository_is_never_adopted() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    std::fs::create_dir_all(&receiver.path).unwrap();
    git_in(&receiver.path, &["init", "--bare"]);
    let config = std::fs::read(receiver.path.join("config")).unwrap();
    assert!(ensure_receiver(&receiver)
        .unwrap_err()
        .contains("ownership"));
    assert_eq!(config, std::fs::read(receiver.path.join("config")).unwrap());
}

#[test]
fn receiver_ownership_changes_are_refused() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    std::fs::write(receiver.path.join(".build-review-receiver.json"), "{}").unwrap();
    assert!(ensure_receiver(&receiver)
        .unwrap_err()
        .contains("ownership"));
}

#[test]
fn an_owned_initialization_reservation_recovers_an_empty_receiver() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    let parent = receiver.path.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    std::fs::write(
        parent.join(format!(
            ".{}.receiver-reservation.json",
            receiver.repository_id
        )),
        serde_json::to_vec(&ownership(&receiver)).unwrap(),
    )
    .unwrap();
    std::fs::create_dir(&receiver.path).unwrap();
    ensure_receiver(&receiver).unwrap();
    assert!(git2::Repository::open_bare(&receiver.path)
        .unwrap()
        .is_bare());
}

#[test]
fn an_unmarked_empty_directory_is_not_an_owned_reservation() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    std::fs::create_dir_all(&receiver.path).unwrap();
    assert!(ensure_receiver(&receiver)
        .unwrap_err()
        .contains("ownership"));
}

#[cfg(unix)]
#[test]
fn receiver_path_cannot_redirect_through_a_symlink() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    std::fs::create_dir_all(receiver.path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(source.join(".git"), &receiver.path).unwrap();
    assert!(ensure_receiver(&receiver).unwrap_err().contains("symlink"));
}

#[cfg(unix)]
fn interrupt_config_writer(source: &Path) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reviews::receivers::tests::interrupted_config_transaction_recovers_only_its_own_lock",
            "--nocapture",
        ])
        .env("BUILD_REVIEW_INTERRUPTED_CONFIG_DIR", source)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(23));
}

#[cfg(unix)]
#[test]
fn interrupted_config_transaction_recovers_only_its_own_lock() {
    if let Some(path) = std::env::var_os("BUILD_REVIEW_INTERRUPTED_CONFIG_DIR") {
        with_local_config_locked(Path::new(&path), |_, local| {
            local.set_str("build.interrupted", "true").unwrap();
            std::process::exit(23);
        })
        .unwrap();
        panic!("interrupted writer must exit inside the transaction");
    }
    let (_temporary, source) = init_repo();
    let config_path = source.join(".git/config");
    let before = std::fs::read(&config_path).unwrap();
    interrupt_config_writer(&source);
    assert!(source.join(".git/config.lock").exists());
    assert_eq!(std::fs::read(&config_path).unwrap(), before);
    with_local_config_locked(&source, |_, local| {
        local
            .set_str("build.recovered", "true")
            .map_err(|error| error.to_string())
    })
    .unwrap();
    assert!(!source.join(".git/config.lock").exists());
    assert!(git2::Repository::open(&source)
        .unwrap()
        .config()
        .unwrap()
        .get_bool("build.recovered")
        .unwrap());
    interrupt_config_writer(&source);
    std::fs::remove_file(source.join(".git/config.lock")).unwrap();
    std::fs::write(source.join(".git/config.lock"), "user lock\n").unwrap();
    let error = with_local_config_locked(&source, |_, _| {
        panic!("a replaced user lock must remain untouched")
    })
    .unwrap_err();
    assert!(error.contains("prove it owns"));
    assert_eq!(
        std::fs::read_to_string(source.join(".git/config.lock")).unwrap(),
        "user lock\n"
    );
}

#[test]
fn receiver_recovers_an_interrupted_atomic_marker_write() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    let parent = receiver.path.parent().unwrap();
    std::fs::create_dir_all(&receiver.path).unwrap();
    write_owned_json(
        &parent.join(format!(
            ".{}.receiver-reservation.json",
            receiver.repository_id
        )),
        &ownership(&receiver),
    )
    .unwrap();
    std::fs::write(
        receiver
            .path
            .join(format!(".build-review-write-{}.tmp", uuid::Uuid::new_v4())),
        "{partial",
    )
    .unwrap();
    ensure_receiver(&receiver).unwrap();
}

#[test]
fn configuration_commit_preserves_a_lock_replaced_while_held() {
    let (_temporary, source) = init_repo();
    let config = source.join(".git/config");
    let before = std::fs::read(&config).unwrap();
    let lock = source.join(".git/config.lock");
    let error = with_local_config_locked(&source, |_, staged| {
        staged
            .set_str("build.staged", "true")
            .map_err(|error| error.to_string())?;
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&lock, "user replacement\n").unwrap();
        Ok(())
    })
    .unwrap_err();
    assert!(error.contains("replaced"));
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        "user replacement\n"
    );
}
