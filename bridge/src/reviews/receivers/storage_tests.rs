use super::*;
use crate::git_fixture::{git_in, init_repo};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[test]
#[cfg(unix)]
fn receiver_links_history_and_survives_gc_source_removal_and_native_push() {
    let (temporary, source) = init_repo();
    git_in(&source, &["gc", "--prune=now"]);
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    let pack = fs::read_dir(source.join(".git/objects/pack"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .unwrap();
    let linked = receiver
        .path
        .join("objects/pack")
        .join(pack.file_name().unwrap());
    assert!(
        linked.exists(),
        "receiver should share immutable source packs"
    );
    assert_eq!(
        fs::metadata(&pack).unwrap().ino(),
        fs::metadata(&linked).unwrap().ino()
    );
    assert!(linked.with_extension("keep").exists());
    let original_head = git(&source, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let source_index = pack.with_extension("idx");
    let source_index_bytes = fs::read(&source_index).unwrap();
    assert_ne!(
        fs::metadata(&source_index).unwrap().ino(),
        fs::metadata(linked.with_extension("idx")).unwrap().ino()
    );
    fs::remove_file(linked.with_extension("idx")).unwrap();
    git_in(&receiver.path, &["index-pack", linked.to_str().unwrap()]);
    assert_eq!(fs::read(&source_index).unwrap(), source_index_bytes);
    git_in(
        &source,
        &[
            "push",
            receiver.path.to_str().unwrap(),
            "main:refs/heads/review",
        ],
    );
    fs::write(source.join("new.txt"), "new review work\n").unwrap();
    git_in(&source, &["add", "."]);
    git_in(&source, &["commit", "-m", "review work"]);
    git_in(
        &source,
        &[
            "push",
            receiver.path.to_str().unwrap(),
            "main:refs/heads/review",
        ],
    );
    let repository = git2::Repository::open_bare(&receiver.path).unwrap();
    let head = repository.refname_to_id("refs/heads/review").unwrap();
    git_in(&source, &["checkout", "--orphan", "replacement"]);
    git_in(&source, &["commit", "--allow-empty", "-m", "replacement"]);
    git_in(&source, &["branch", "-D", "main"]);
    git_in(&source, &["reflog", "expire", "--expire=now", "--all"]);
    git_in(&source, &["repack", "-ad"]);
    git_in(&source, &["gc", "--prune=now"]);
    git_in(&receiver.path, &["gc", "--prune=now"]);
    assert_eq!(
        fs::metadata(&pack).ok().map(|metadata| metadata.ino()),
        None
    );
    assert!(linked.exists());
    for entry in fs::read_dir(receiver.path.join("objects/pack")).unwrap() {
        let index = entry.unwrap().path();
        if index
            .extension()
            .is_some_and(|extension| extension == "idx")
            && index != linked.with_extension("idx")
        {
            let contents = git(
                &receiver.path,
                &["verify-pack", "-v", index.to_str().unwrap()],
            )
            .unwrap();
            assert!(
                !contents
                    .lines()
                    .any(|line| line.starts_with(&original_head)),
                "receiver gc must not duplicate shared history"
            );
        }
    }
    assert_eq!(fs::metadata(&linked).unwrap().nlink(), 1);
    fs::remove_dir_all(&source).unwrap();
    git_in(&receiver.path, &["fsck", "--full"]);
    let commit = repository.find_commit(head).unwrap();
    let tree = commit.tree().unwrap();
    let blob = tree
        .get_name("new.txt")
        .unwrap()
        .to_object(&repository)
        .unwrap();
    assert_eq!(blob.as_blob().unwrap().content(), b"new review work\n");
}

#[test]
fn incomplete_source_graph_falls_back_to_an_empty_receiver() {
    let (temporary, source) = init_repo();
    let repository = git2::Repository::open(&source).unwrap();
    let tree = repository
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree_id()
        .to_string();
    fs::remove_file(
        source
            .join(".git/objects")
            .join(&tree[..2])
            .join(&tree[2..]),
    )
    .unwrap();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    let destination = git2::Repository::open_bare(&receiver.path).unwrap();
    assert!(destination
        .find_commit(repository.head().unwrap().target().unwrap())
        .is_err());
}

#[test]
#[cfg(unix)]
fn source_object_symlinks_and_unrecognized_files_are_not_imported() {
    let (temporary, source) = init_repo();
    let objects = source.join(".git/objects");
    let secret = temporary.path().join("not-an-object");
    fs::write(&secret, "private data").unwrap();
    fs::create_dir_all(objects.join("ab")).unwrap();
    std::os::unix::fs::symlink(&secret, objects.join("ab").join("c".repeat(38))).unwrap();
    fs::write(objects.join("ab/unrecognized"), "private data").unwrap();
    fs::write(objects.join("pack/arbitrary.pack"), "private data").unwrap();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    assert!(!receiver
        .path
        .join("objects/ab")
        .join("c".repeat(38))
        .exists());
    assert!(!receiver.path.join("objects/ab/unrecognized").exists());
    assert!(!receiver.path.join("objects/pack/arbitrary.pack").exists());
}

#[test]
#[cfg(unix)]
fn receiver_links_loose_objects_without_copying_source_refs_or_config() {
    let (temporary, source) = init_repo();
    let repository = git2::Repository::open(&source).unwrap();
    let head = repository.head().unwrap().target().unwrap().to_string();
    let relative = PathBuf::from(&head[..2]).join(&head[2..]);
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    let original = source.join(".git/objects").join(&relative);
    let linked = receiver.path.join("objects").join(relative);
    assert!(linked.exists());
    assert_eq!(
        fs::metadata(original).unwrap().ino(),
        fs::metadata(linked).unwrap().ino()
    );
    let destination = git2::Repository::open_bare(&receiver.path).unwrap();
    assert_eq!(destination.references().unwrap().count(), 0);
    assert!(git2::Config::open(&receiver.path.join("config"))
        .unwrap()
        .get_string("user.email")
        .is_err());
}

#[test]
#[cfg(unix)]
fn shared_pack_keeps_release_only_owned_markers_after_source_unlinks() {
    let (temporary, source) = init_repo();
    git_in(&source, &["gc", "--prune=now"]);
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    let pack = fs::read_dir(receiver.path.join("objects/pack"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "pack")
        })
        .unwrap();
    let keep = pack.with_extension("keep");
    let marker = fs::read(&keep).unwrap();
    storage::release_unshared_pack_keeps(&receiver.path).unwrap();
    assert!(keep.exists(), "source still shares this pack");
    fs::remove_dir_all(source).unwrap();
    fs::write(&keep, b"user keep marker\n").unwrap();
    storage::release_unshared_pack_keeps(&receiver.path).unwrap();
    assert_eq!(fs::read(&keep).unwrap(), b"user keep marker\n");
    fs::write(&keep, marker).unwrap();
    storage::release_unshared_pack_keeps(&receiver.path).unwrap();
    assert!(!keep.exists());
    git_in(&receiver.path, &["gc", "--prune=now"]);
    assert!(
        !pack.exists(),
        "unreferenced private history should be reclaimed"
    );
}

#[test]
fn unsupported_object_links_fall_back_to_normal_self_contained_publication() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    initialization::ensure_with(&receiver, |attempt| {
        git(&attempt.path, &["init", "--bare"])?;
        storage::seed_with(attempt, |_, _| {
            Err(std::io::Error::from_raw_os_error(libc::EXDEV))
        })
    })
    .unwrap();
    let destination = git2::Repository::open_bare(&receiver.path).unwrap();
    let head = git2::Repository::open(&source)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap();
    assert!(destination.find_commit(head).is_err());
    git_in(
        &source,
        &[
            "push",
            receiver.path.to_str().unwrap(),
            "main:refs/heads/review",
        ],
    );
    fs::remove_dir_all(source).unwrap();
    git_in(&receiver.path, &["gc", "--prune=now"]);
    git_in(&receiver.path, &["fsck", "--full"]);
    assert!(destination.find_commit(head).is_ok());
}
