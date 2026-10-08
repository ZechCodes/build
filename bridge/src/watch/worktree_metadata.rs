//! Shared Git metadata coverage for checkout subscriptions. Directory traversal
//! excludes object databases, other worktrees and private refs before watching.
use notify::{Event, EventKind, Watcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Registry = Mutex<BTreeMap<PathBuf, Weak<MetadataTree>>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();
static NEXT_SUBSCRIBER: AtomicU64 = AtomicU64::new(1);

pub(super) struct MetadataSubscription {
    tree: Arc<MetadataTree>,
    subscriber: u64,
}
impl MetadataSubscription {
    pub(super) fn coverage(&self) -> usize {
        self.tree.registered.lock().unwrap().len()
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn root(&self) -> &Path {
        &self.tree.root
    }
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn pause_delivery(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Sender<Event>>> {
        self.tree.subscribers.lock().unwrap()
    }
}
impl Drop for MetadataSubscription {
    fn drop(&mut self) {
        self.tree
            .subscribers
            .lock()
            .unwrap()
            .remove(&self.subscriber);
    }
}

pub(super) fn subscribe(
    root: &Path,
    sender: Sender<Event>,
) -> Result<MetadataSubscription, String> {
    let mut registry = REGISTRY.get_or_init(Registry::default).lock().unwrap();
    registry.retain(|_, tree| tree.strong_count() > 0);
    let tree = match registry.get(root).and_then(Weak::upgrade) {
        Some(tree) => tree,
        None => {
            let tree = MetadataTree::start(root)?;
            registry.insert(root.to_path_buf(), Arc::downgrade(&tree));
            tree
        }
    };
    let subscriber = NEXT_SUBSCRIBER.fetch_add(1, Ordering::Relaxed);
    tree.subscribers.lock().unwrap().insert(subscriber, sender);
    Ok(MetadataSubscription { tree, subscriber })
}

struct MetadataTree {
    root: PathBuf,
    watcher: Mutex<notify::RecommendedWatcher>,
    registered: Mutex<BTreeSet<PathBuf>>,
    subscribers: Mutex<BTreeMap<u64, Sender<Event>>>,
}
impl MetadataTree {
    fn start(root: &Path) -> Result<Arc<Self>, String> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if let Ok(event) = result {
                let _ = sender.send(event);
            }
        })
        .map_err(|error| error.to_string())?;
        let tree = Arc::new(Self {
            root: root.to_path_buf(),
            watcher: Mutex::new(watcher),
            registered: Mutex::new(BTreeSet::new()),
            subscribers: Mutex::new(BTreeMap::new()),
        });
        tree.watch_directory(root)?;
        let weak = Arc::downgrade(&tree);
        std::thread::spawn(move || forward(receiver, weak));
        Ok(tree)
    }

    fn watch_directory(&self, path: &Path) -> Result<(), String> {
        if !self.includes(path) {
            return Ok(());
        }
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() {
            return Ok(());
        }
        self.register(path)?;
        for entry in std::fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                self.watch_directory(&entry.path())?;
            }
        }
        Ok(())
    }

    fn register(&self, path: &Path) -> Result<(), String> {
        let mut registered = self.registered.lock().unwrap();
        if registered.contains(path) {
            return Ok(());
        }
        self.watcher
            .lock()
            .unwrap()
            .watch(path, notify::RecursiveMode::NonRecursive)
            .map_err(|error| error.to_string())?;
        registered.insert(path.to_path_buf());
        Ok(())
    }

    fn includes(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| !excluded(relative))
    }

    fn update_directories(&self, event: &Event) {
        for (index, path) in event.paths.iter().enumerate() {
            let exists = path.exists();
            // Removal can reach us after the same path has already been
            // recreated. Forget the old subtree before adopting its new inode.
            let removed = removal(&event.kind, index) || !exists;
            if removed {
                self.forget(path);
            }
            if exists && (removed || super::may_add_a_directory(&event.kind)) {
                if let Err(error) = self.watch_directory(path) {
                    crate::logline::say(format!("watch: metadata {}: {error}", path.display()));
                }
            }
        }
    }

    fn forget(&self, path: &Path) {
        let mut registered = self.registered.lock().unwrap();
        let removed: Vec<_> = registered
            .iter()
            .filter(|entry| entry.starts_with(path))
            .cloned()
            .collect();
        let mut watcher = self.watcher.lock().unwrap();
        for entry in removed {
            let _ = watcher.unwatch(&entry);
            registered.remove(&entry);
        }
    }

    fn deliver(&self, mut event: Event) {
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        event.paths.retain(|path| self.includes(path));
        if event.paths.is_empty() {
            return;
        }
        self.update_directories(&event);
        self.subscribers
            .lock()
            .unwrap()
            .retain(|_, sender| sender.send(event.clone()).is_ok());
    }
}

fn removal(kind: &EventKind, index: usize) -> bool {
    use notify::event::{ModifyKind, RenameMode};
    match kind {
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => true,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => index == 0,
        _ => false,
    }
}

fn forward(receiver: Receiver<Event>, tree: Weak<MetadataTree>) {
    while let Ok(event) = receiver.recv() {
        let Some(tree) = tree.upgrade() else {
            return;
        };
        tree.deliver(event);
    }
}

/// Keep public/custom ref namespaces while excluding Build's private retained
/// history and temporary imports, including their reflog counterparts.
pub(super) fn excluded(relative: &Path) -> bool {
    let bookkeeping = relative
        .iter()
        .next()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".build-review-"));
    bookkeeping
        || [
            "objects",
            "worktrees",
            "build-review-snapshots",
            "build-review-sync",
            "refs/build",
            "logs/refs/build",
        ]
        .into_iter()
        .any(|private| relative.starts_with(private))
}
