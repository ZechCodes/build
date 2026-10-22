//! Filesystem root and destination serialization, independent of Git work.
//! Root and destination keys also have separate namespaces, so a directory
//! destination cannot alias the root lock already held by the same operation.
use crate::api::ApiError;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Instant;

type LockRegistry = (Mutex<HashSet<PathBuf>>, Condvar);

fn roots() -> &'static LockRegistry {
    static HELD: OnceLock<LockRegistry> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

fn destinations() -> &'static LockRegistry {
    static HELD: OnceLock<LockRegistry> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

pub(in crate::app) struct RootLock {
    _held: PathLock,
}

impl RootLock {
    /// The caller has canonicalized the root outside the app mutex.
    pub(in crate::app) fn acquire(root: &Path, deadline: Instant) -> Result<Self, ApiError> {
        Ok(Self {
            _held: PathLock::acquire(roots(), root.to_path_buf(), deadline)?,
        })
    }
}

pub(super) struct DestinationLock {
    _held: PathLock,
}

impl DestinationLock {
    /// The root lock is already held. Share its acquisition deadline;
    /// all path resolution and contention waits stay outside the app mutex.
    pub(super) fn acquire(
        root: &Path,
        relative: &str,
        deadline: Instant,
    ) -> Result<Self, ApiError> {
        Ok(Self {
            _held: PathLock::acquire(destinations(), destination_key(root, relative)?, deadline)?,
        })
    }
}

struct PathLock {
    registry: &'static LockRegistry,
    path: PathBuf,
}

impl PathLock {
    fn acquire(
        registry: &'static LockRegistry,
        path: PathBuf,
        deadline: Instant,
    ) -> Result<Self, ApiError> {
        let (held, released) = registry;
        let mut held = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while held.contains(&path) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ApiError::busy(
                    "Another filesystem operation is still running; try again.",
                ));
            }
            held = released
                .wait_timeout(held, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        held.insert(path.clone());
        Ok(Self { registry, path })
    }
}

/// Resolve aliases of the containing directory without following the leaf.
/// This is only a serialization key: the descriptor-relative service still
/// enforces authority and reports invalid/missing paths in its usual way.
fn destination_key(root: &Path, relative: &str) -> Result<PathBuf, ApiError> {
    if !crate::fs_scope::is_worktree_contained_path(relative) {
        return Err(ApiError::classify("path escapes the worktree".into()));
    }
    let target = root.join(relative);
    let parent = target.parent().unwrap_or(root);
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    Ok(match target.file_name() {
        Some(name) => parent.join(name),
        None => target,
    })
}

impl Drop for PathLock {
    fn drop(&mut self) {
        let (held, released) = self.registry;
        held.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
        released.notify_all();
    }
}

fn source_uses() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static USES: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    USES.get_or_init(Default::default)
}

/// Tracks recorded roots without canonicalization or I/O in metadata guards.
pub(super) struct SourceUse {
    recorded_root: PathBuf,
}

impl SourceUse {
    pub(super) fn acquire(recorded_root: &Path) -> Self {
        let mut uses = source_uses()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *uses.entry(recorded_root.to_path_buf()).or_default() += 1;
        Self {
            recorded_root: recorded_root.to_path_buf(),
        }
    }
}

pub(super) fn source_in_use(recorded_root: &Path) -> bool {
    source_uses()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains_key(recorded_root)
}

impl Drop for SourceUse {
    fn drop(&mut self) {
        let mut uses = source_uses()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = uses
            .get_mut(&self.recorded_root)
            .expect("registered source");
        *count -= 1;
        if *count == 0 {
            uses.remove(&self.recorded_root);
        }
    }
}
