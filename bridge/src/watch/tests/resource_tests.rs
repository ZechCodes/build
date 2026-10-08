use super::*;
use std::os::unix::fs::MetadataExt;

fn fixture() -> (tempfile::TempDir, git2::Repository, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path().join("source")).unwrap();
    let signature = git2::Signature::now("Test", "test@example.com").unwrap();
    let tree = repo.index().unwrap().write_tree().unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "initial",
        &repo.find_tree(tree).unwrap(),
        &[],
    )
    .unwrap();
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    repo.worktree("first", &first, None).unwrap();
    repo.worktree("second", &second, None).unwrap();
    std::fs::create_dir_all(repo.path().join("refs/build")).unwrap();
    std::fs::create_dir_all(repo.path().join("logs/refs/build")).unwrap();
    for linked in [&first, &second] {
        let linked_repo = git2::Repository::open(linked).unwrap();
        std::fs::create_dir_all(linked_repo.path().join("refs/build")).unwrap();
        std::fs::create_dir_all(linked_repo.path().join("logs/refs/build")).unwrap();
    }
    (dir, repo, first, second)
}

fn directory_inodes(path: &Path, excluded: &Path, inodes: &mut BTreeSet<u64>) {
    if path == excluded {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    inodes.insert(metadata.ino());
    for entry in std::fs::read_dir(path).unwrap().flatten() {
        directory_inodes(&entry.path(), excluded, inodes);
    }
}

fn watched_inode(line: &str) -> Option<u64> {
    if !line.starts_with("inotify wd:") {
        return None;
    }
    let inode = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("ino:"))?;
    u64::from_str_radix(inode, 16).ok()
}

fn kernel_watch_count(root: &Path) -> usize {
    let mut inodes = BTreeSet::new();
    directory_inodes(root, &root.join("worktrees"), &mut inodes);
    std::fs::read_dir("/proc/self/fdinfo")
        .unwrap()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .map(|info| {
            info.lines()
                .filter_map(watched_inode)
                .filter(|inode| inodes.contains(inode))
                .count()
        })
        .sum()
}

fn private_snapshots(root: &Path) {
    for name in ["build-review-snapshots", "build-review-sync"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for snapshot in 0..128 {
        for prefix in ["refs/build/reviews", "logs/refs/build/reviews"] {
            let path = root.join(format!("{prefix}/task/snapshot-{snapshot}/directory"));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("head"), "pin").unwrap();
        }
        for name in ["build-review-snapshots", "build-review-sync"] {
            std::fs::write(
                root.join(format!("{name}/snapshot-{snapshot}.json")),
                "marker",
            )
            .unwrap();
        }
        std::fs::write(
            root.join(format!(".build-review-marker-{snapshot}.json")),
            "marker",
        )
        .unwrap();
    }
}

fn private_count(root: &Path) -> usize {
    kernel_watch_count(&root.join("refs/build"))
        + kernel_watch_count(&root.join("logs/refs/build"))
        + kernel_watch_count(&root.join("build-review-snapshots"))
        + kernel_watch_count(&root.join("build-review-sync"))
}

#[test]
fn existing_private_snapshots_never_receive_kernel_watches() {
    let (_dir, repo, first, _) = fixture();
    let gitdir = git2::Repository::open(&first).unwrap().path().to_path_buf();
    private_snapshots(repo.commondir());
    private_snapshots(&gitdir);
    let (_watcher, sink) = start_recording(&first);
    assert_eq!(private_count(repo.commondir()), 0);
    assert_eq!(private_count(&gitdir), 0);
    std::fs::write(repo.path().join("refs/heads/public"), "branch").unwrap();
    assert!(settle(&sink, |sink| sink.git_notes() > 0));
    assert!(sink.noted_paths().is_empty());
}

#[test]
fn growing_private_snapshots_do_not_grow_worktree_kernel_coverage() {
    let (_dir, repo, first, _) = fixture();
    let (_watcher, sink) = start_recording(&first);
    let gitdir = git2::Repository::open(&first).unwrap().path().to_path_buf();
    let before = kernel_watch_count(repo.commondir());
    let local_before = kernel_watch_count(&gitdir);
    private_snapshots(repo.commondir());
    private_snapshots(&gitdir);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(kernel_watch_count(repo.commondir()), before);
    assert_eq!(kernel_watch_count(&gitdir), local_before);
    assert_eq!(private_count(repo.commondir()), 0);
    assert_eq!(private_count(&gitdir), 0);
    assert_eq!(sink.git_notes(), 0);
}

#[test]
fn linked_checkouts_share_common_kernel_watches_and_receive_independent_events() {
    let (_dir, repo, first, second) = fixture();
    let (first_watcher, first_sink) = start_recording(&first);
    let before = kernel_watch_count(repo.commondir());
    let (second_watcher, second_sink) = start_recording(&second);
    assert_eq!(kernel_watch_count(repo.commondir()), before);
    first_sink.forget();
    second_sink.forget();
    std::fs::write(repo.path().join("refs/heads/shared"), "branch").unwrap();
    assert!(settle(&first_sink, |sink| sink.git_notes() > 0));
    assert!(settle(&second_sink, |sink| sink.git_notes() > 0));
    assert!(first_sink.noted_paths().is_empty());
    assert!(second_sink.noted_paths().is_empty());
    drop(first_watcher);
    std::thread::sleep(Duration::from_millis(150));
    second_sink.forget();
    first_sink.forget();
    let second_repo = git2::Repository::open(&second).unwrap();
    std::fs::write(second_repo.path().join("HEAD"), "ref: refs/heads/second\n").unwrap();
    assert!(settle(&second_sink, |sink| sink.git_notes() > 0));
    assert_eq!(kernel_watch_count(repo.commondir()), before);
    assert_eq!(
        first_sink.git_notes(),
        0,
        "dropped subscriber receives nothing"
    );
    drop(second_watcher);
    assert!(settle(&second_sink, |_| kernel_watch_count(
        repo.commondir()
    ) == 0));
}

#[test]
fn primary_checkout_skips_private_git_trees_before_registration() {
    let (_dir, repo, _, _) = fixture();
    private_snapshots(repo.commondir());
    let (_watcher, sink) = start_recording(repo.workdir().unwrap());
    assert_eq!(private_count(repo.commondir()), 0);
    assert_eq!(kernel_watch_count(&repo.path().join("objects")), 0);
    std::fs::write(repo.path().join("index"), "dirty").unwrap();
    assert!(settle(&sink, |sink| sink.git_notes() > 0));
    assert!(sink.noted_paths().is_empty());
}

#[test]
fn public_metadata_remains_observed_by_each_linked_checkout() {
    let (_dir, repo, first, second) = fixture();
    for path in [
        "refs/remotes/build-review/topic",
        "refs/tags/version",
        "refs/custom/label",
    ] {
        std::fs::create_dir_all(repo.path().join(path).parent().unwrap()).unwrap();
    }
    let (_first_watcher, first_sink) = start_recording(&first);
    let (_second_watcher, second_sink) = start_recording(&second);
    for path in [
        "refs/remotes/build-review/topic",
        "refs/tags/version",
        "refs/custom/label",
        "logs/HEAD",
        "packed-refs",
    ] {
        first_sink.forget();
        second_sink.forget();
        let content = if path == "packed-refs" {
            "# pack-refs with: peeled\n"
        } else {
            "metadata"
        };
        std::fs::write(repo.path().join(path), content).unwrap();
        assert!(settle(&first_sink, |sink| sink.git_notes() > 0), "{path}");
        assert!(settle(&second_sink, |sink| sink.git_notes() > 0), "{path}");
        assert!(first_sink.noted_paths().is_empty());
        assert!(second_sink.noted_paths().is_empty());
        std::thread::sleep(Duration::from_millis(100));
    }
    let local = git2::Repository::open(&first).unwrap();
    for path in ["index", "MERGE_HEAD"] {
        first_sink.forget();
        second_sink.forget();
        std::fs::write(local.path().join(path), "metadata").unwrap();
        assert!(settle(&first_sink, |sink| sink.git_notes() > 0), "{path}");
        assert_eq!(
            second_sink.git_notes(),
            0,
            "{path} belongs to first checkout"
        );
        assert!(first_sink.noted_paths().is_empty());
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn rapid_heads_replacement_reinstalls_current_directory_coverage() {
    let (_dir, repo, first, second) = fixture();
    std::fs::create_dir(repo.path().join("refs/heads/topic")).unwrap();
    let (first_watcher, first_sink) = start_recording(&first);
    let (_second_watcher, second_sink) = start_recording(&second);
    let common = repo.commondir().canonicalize().unwrap();
    let subscription = first_watcher
        ._metadata
        .iter()
        .find(|subscription| subscription.root() == common)
        .unwrap();
    let before = subscription.coverage();
    // Queue a normal event before replacement and hold its fanout. The worker
    // then processes removal after the new directory already exists.
    let pause = subscription.pause_delivery();
    std::fs::write(common.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::remove_dir_all(common.join("refs/heads")).unwrap();
    std::fs::create_dir(common.join("refs/heads")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    drop(pause);
    std::thread::sleep(Duration::from_millis(300));
    first_sink.forget();
    second_sink.forget();
    std::fs::write(common.join("refs/heads/restored"), "branch").unwrap();
    assert!(settle(&first_sink, |sink| sink.git_notes() > 0));
    assert!(settle(&second_sink, |sink| sink.git_notes() > 0));
    assert!(first_sink.noted_paths().is_empty());
    assert!(second_sink.noted_paths().is_empty());
    assert_eq!(
        subscription.coverage(),
        before - 1,
        "deleted topic subtree is pruned"
    );
}
