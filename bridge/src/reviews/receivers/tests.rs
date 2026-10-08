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
