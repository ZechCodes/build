//! Destination serialization shared by overlapping file scopes.
//!
//! Repository locks use common Git directories. These keys have a separate
//! namespace: a target which became a directory cannot alias its repository lock.
use crate::api::ApiError;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Instant;

fn destinations() -> &'static (Mutex<HashSet<PathBuf>>, Condvar) {
    static HELD: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

pub(super) struct DestinationLock {
    path: PathBuf,
}

impl DestinationLock {
    /// The repository lock is already held. Share its acquisition deadline;
    /// all path resolution and contention waits stay outside the app mutex.
    pub(super) fn acquire(
        root: &Path,
        relative: &str,
        deadline: Instant,
    ) -> Result<Self, ApiError> {
        let path = destination_key(root, relative)?;
        let (held, released) = destinations();
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
        Ok(Self { path })
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

impl Drop for DestinationLock {
    fn drop(&mut self) {
        let (held, released) = destinations();
        held.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
        released.notify_all();
    }
}
