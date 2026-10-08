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
            if self.running.contains_key(&identity) {
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
    watcher
        .watch(identity, notify::RecursiveMode::NonRecursive)
        .map_err(|error| watch_error(identity, error))?;
    let refs = identity.join("refs");
    if refs.is_dir() {
        watcher
            .watch(&refs, notify::RecursiveMode::Recursive)
            .map_err(|error| watch_error(identity, error))?;
    }
    Ok(watcher)
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
                &[root.clone()],
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
