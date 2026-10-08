//! Subscription-independent committed-ref notifications. Callbacks only enqueue
//! canonical metadata identities; repository reads belong to the consumer.
use super::WatchError;
use notify::Watcher;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An enqueue operation receiving a canonical metadata-directory identity.
pub type MetadataCallback = Arc<dyn Fn(PathBuf) + Send + Sync>;

/// One backend watcher per canonical metadata directory, independent of clients.
#[derive(Default)]
pub struct MetadataWatchers {
    running: BTreeMap<PathBuf, notify::RecommendedWatcher>,
}

impl MetadataWatchers {
    /// Reconcile off the app lock. Missing repositories or watch failures are
    /// returned for the caller to report while its polling fallback remains live.
    pub fn reconcile(
        &mut self,
        repositories: &[PathBuf],
        enqueue: MetadataCallback,
    ) -> Vec<WatchError> {
        let mut wanted = BTreeSet::new();
        let mut errors = Vec::new();
        for repository in repositories {
            match metadata_roots(repository) {
                Ok(roots) => wanted.extend(roots),
                Err(error) => errors.push(error),
            }
        }
        self.running.retain(|identity, _| wanted.contains(identity));
        for identity in wanted {
            if let Some(watcher) = self.running.get_mut(&identity) {
                if let Err(error) = register_metadata(watcher, &identity) {
                    errors.push(error);
                }
                continue;
            }
            match start(&identity, Arc::clone(&enqueue)) {
                Ok(watcher) => {
                    self.running.insert(identity, watcher);
                }
                Err(error) => errors.push(error),
            }
        }
        errors
    }
}

/// Resolve both linked-worktree Git metadata and its shared common directory.
/// Bare repositories produce one identity.
pub fn metadata_roots(repository: &Path) -> Result<BTreeSet<PathBuf>, WatchError> {
    let fail = |message| WatchError::Notify {
        path: repository.display().to_string(),
        message,
    };
    let git = git2::Repository::open(repository).map_err(|error| fail(error.to_string()))?;
    [git.path(), git.commondir()]
        .into_iter()
        .map(|path| path.canonicalize().map_err(|error| fail(error.to_string())))
        .collect()
}

fn start(
    identity: &Path,
    enqueue: MetadataCallback,
) -> Result<notify::RecommendedWatcher, WatchError> {
    let root = identity.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            if relevant_event(&root, &event) {
                enqueue(root.clone());
            }
        }
    })
    .map_err(|error| watch_error(identity, error))?;
    register_metadata(&mut watcher, identity)?;
    Ok(watcher)
}

/// Private snapshot refs can retain arbitrarily many directories. Only branch
/// directories carry recursive watches. Re-registering on each reconcile also
/// recovers a refs/heads directory replaced since the previous scan.
fn register_metadata(
    watcher: &mut notify::RecommendedWatcher,
    identity: &Path,
) -> Result<(), WatchError> {
    watcher
        .watch(identity, notify::RecursiveMode::NonRecursive)
        .map_err(|error| watch_error(identity, error))?;
    for (relative, mode) in [
        ("refs", notify::RecursiveMode::NonRecursive),
        ("refs/heads", notify::RecursiveMode::Recursive),
    ] {
        let path = identity.join(relative);
        if path.is_dir() {
            watcher
                .watch(&path, mode)
                .map_err(|error| watch_error(identity, error))?;
        } else {
            // The backend may already have removed a vanished directory.
            let _ = watcher.unwatch(&path);
        }
    }
    Ok(())
}

fn watch_error(path: &Path, error: notify::Error) -> WatchError {
    WatchError::Notify {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}

fn relevant_event(root: &Path, event: &notify::Event) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|path| relevant(root, path))
}

fn relevant(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if path
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return false;
    }
    relative == Path::new("HEAD")
        || relative == Path::new("packed-refs")
        || relative.starts_with("refs/heads")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_classification_ignores_dirty_objects_and_pins() {
        let root = Path::new("/repo/.git");
        for path in ["HEAD", "packed-refs", "refs/heads/main"] {
            assert!(relevant(root, &root.join(path)), "{path}");
        }
        for path in [
            "index",
            "index.lock",
            "objects/ab/cdef",
            "refs/heads/main.lock",
            "refs/build/reviews/a/snapshot/head",
            "refs/remotes/origin/main",
            "logs/HEAD",
        ] {
            assert!(!relevant(root, &root.join(path)), "{path}");
        }
    }
    #[test]
    fn linked_worktrees_resolve_external_gitdir_and_shared_common_dir() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path().join("source")).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "initial",
            &repo.find_tree(tree_id).unwrap(),
            &[],
        )
        .unwrap();
        let linked = dir.path().join("linked");
        repo.worktree("linked", &linked, None).unwrap();
        let roots = metadata_roots(&linked).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&repo.commondir().canonicalize().unwrap()));
        assert!(roots.contains(&repo.path().join("worktrees/linked").canonicalize().unwrap()));
        let mut service = MetadataWatchers::default();
        assert!(service
            .reconcile(
                &[linked.clone(), linked, repo.workdir().unwrap().into()],
                Arc::new(|_| {})
            )
            .is_empty());
        assert_eq!(service.running.len(), 2);
        service.reconcile(&[], Arc::new(|_| {}));
        assert!(service.running.is_empty());
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use notify::event::AccessKind;
    #[test]
    fn metadata_reads_do_not_enqueue_another_sync() {
        let event = notify::Event::new(notify::EventKind::Access(AccessKind::Read))
            .add_path(PathBuf::from("/repo/HEAD"));
        assert!(!relevant_event(Path::new("/repo"), &event));
    }
    #[test]
    fn bare_receiver_ref_and_packed_ref_writes_enqueue_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init_bare(dir.path().join("receiver.git")).unwrap();
        let root = repo.path().canonicalize().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut service = MetadataWatchers::default();
        assert!(service
            .reconcile(
                std::slice::from_ref(&root),
                Arc::new(move |id| {
                    let _ = tx.send(id);
                })
            )
            .is_empty());
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(
            root.join("refs/heads/review"),
            "0000000000000000000000000000000000000000\n",
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            root
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("packed-refs"), "# pack-refs with: peeled\n").unwrap();
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            root
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
        std::fs::create_dir_all(root.join("refs/build/reviews/pin")).unwrap();
        std::fs::write(root.join("refs/build/reviews/pin/head"), "pin").unwrap();
        std::fs::write(root.join("index"), "dirty").unwrap();
        std::fs::create_dir_all(root.join("objects/ab")).unwrap();
        std::fs::write(root.join("objects/ab/oid"), "object").unwrap();
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(250))
            .is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod registration_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::time::Duration;

    fn directory_inodes(root: &Path, inodes: &mut BTreeSet<u64>) {
        let metadata = std::fs::symlink_metadata(root).unwrap();
        if !metadata.is_dir() {
            return;
        }
        inodes.insert(metadata.ino());
        for entry in std::fs::read_dir(root).unwrap().flatten() {
            directory_inodes(&entry.path(), inodes);
        }
    }

    fn kernel_watch_count(root: &Path) -> usize {
        let mut owned = BTreeSet::new();
        directory_inodes(root, &mut owned);
        std::fs::read_dir("/proc/self/fdinfo")
            .unwrap()
            .flatten()
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .map(|info| {
                info.lines()
                    .filter_map(watched_inode)
                    .filter(|inode| owned.contains(inode))
                    .count()
            })
            .sum()
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

    #[test]
    fn private_snapshot_directories_do_not_allocate_kernel_watches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init_bare(dir.path().join("receiver.git")).unwrap();
        let root = repo.path().canonicalize().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let enqueue: MetadataCallback = Arc::new(move |path| {
            let _ = tx.send(path);
        });
        let mut service = MetadataWatchers::default();
        assert!(service
            .reconcile(std::slice::from_ref(&root), enqueue.clone())
            .is_empty());
        let initial = kernel_watch_count(&root);
        for snapshot in 0..128 {
            let pin = root.join(format!(
                "refs/build/reviews/task/snapshot-{snapshot}/directory"
            ));
            std::fs::create_dir_all(&pin).unwrap();
            std::fs::write(pin.join("head"), "pin").unwrap();
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(service
            .reconcile(std::slice::from_ref(&root), enqueue)
            .is_empty());
        assert_eq!(
            kernel_watch_count(&root),
            initial,
            "retained pins consume no watches"
        );
        assert_eq!(initial, 3, "metadata root, refs parent, heads tree");
        assert!(rx.try_recv().is_err(), "pin churn never enqueues a task");
        std::fs::write(root.join("refs/heads/review"), "branch").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), root);
    }

    #[test]
    fn reconcile_reinstalls_a_replaced_heads_directory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init_bare(dir.path().join("receiver.git")).unwrap();
        let root = repo.path().canonicalize().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let enqueue: MetadataCallback = Arc::new(move |path| {
            let _ = tx.send(path);
        });
        let mut service = MetadataWatchers::default();
        assert!(service
            .reconcile(std::slice::from_ref(&root), enqueue.clone())
            .is_empty());
        std::fs::remove_dir(root.join("refs/heads")).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(service
            .reconcile(std::slice::from_ref(&root), enqueue.clone())
            .is_empty());
        assert_eq!(kernel_watch_count(&root), 2);
        std::fs::create_dir(root.join("refs/heads")).unwrap();
        assert!(service
            .reconcile(std::slice::from_ref(&root), enqueue)
            .is_empty());
        std::thread::sleep(Duration::from_millis(100));
        while rx.try_recv().is_ok() {}
        std::fs::write(root.join("refs/heads/review"), "branch").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), root);
        assert_eq!(kernel_watch_count(&root), 3);
    }
}
