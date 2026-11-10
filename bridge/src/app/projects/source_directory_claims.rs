//! A clone owns its destination until settlement; deletion excludes new owners.
use crate::app::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

const WRITING: u8 = 0;
const PREPARED: u8 = 1;
const DELETING: u8 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

impl DirectoryIdentity {
    fn read(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path).ok()?;
        metadata.is_dir().then_some(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

struct DirectoryClaim {
    path: PathBuf,
    phase: AtomicU8,
    identity: Mutex<Option<DirectoryIdentity>>,
}

/// Weak entries keep a dropped or failed operation from retaining a claim.
#[derive(Default)]
pub(in crate::app) struct SourceDirectoryClaims(Mutex<Vec<Weak<DirectoryClaim>>>);

#[derive(Clone)]
pub(in crate::app) struct SourceDirectoryLease(Arc<DirectoryClaim>);

pub(super) fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn live_claims(entries: &mut Vec<Weak<DirectoryClaim>>) -> Vec<Arc<DirectoryClaim>> {
    entries.retain(|entry| entry.strong_count() != 0);
    entries.iter().filter_map(Weak::upgrade).collect()
}

fn new_claim(path: PathBuf, phase: u8) -> Arc<DirectoryClaim> {
    Arc::new(DirectoryClaim {
        identity: Mutex::new(DirectoryIdentity::read(&path)),
        path,
        phase: AtomicU8::new(phase),
    })
}

impl SourceDirectoryClaims {
    pub(in crate::app) fn reserve_destinations(
        &self,
        paths: &[PathBuf],
    ) -> Result<Vec<SourceDirectoryLease>, String> {
        let paths: Vec<_> = paths
            .iter()
            .map(|path| crate::worktree::canonical_planned_path(path))
            .collect();
        let mut entries = self.0.lock().unwrap();
        let active = live_claims(&mut entries);
        let overlaps = paths.iter().enumerate().any(|(index, path)| {
            paths[..index]
                .iter()
                .any(|other| paths_overlap(path, other))
                || active.iter().any(|claim| paths_overlap(path, &claim.path))
        });
        if overlaps {
            return Err("Another source operation owns this clone destination; try again.".into());
        }
        let claims: Vec<_> = paths
            .into_iter()
            .map(|path| new_claim(path, WRITING))
            .collect();
        entries.extend(claims.iter().map(Arc::downgrade));
        Ok(claims.into_iter().map(SourceDirectoryLease).collect())
    }

    /// Prepared directories can be shared by equality; partial batches and
    /// deletion cannot acquire owners. Containment is refused in every phase.
    fn refuse_reference(&self, path: &Path) -> Result<(), String> {
        let path = crate::worktree::canonical_planned_path(path);
        let active = live_claims(&mut self.0.lock().unwrap());
        if active.iter().any(|claim| {
            paths_overlap(&path, &claim.path)
                && (path != claim.path || claim.phase.load(Ordering::SeqCst) != PREPARED)
        }) {
            return Err("A source directory is being prepared or removed; try again.".into());
        }
        Ok(())
    }

    /// Called under the app lock immediately after checking all owners.
    pub(super) fn begin_cleanup(&self, path: &Path) -> Option<SourceDirectoryLease> {
        let mut entries = self.0.lock().unwrap();
        let active = live_claims(&mut entries);
        // Check the original canonical destination before following a later
        // replacement symlink. The clone lease captured the object we made.
        if let Some(claim) = active.iter().find(|claim| claim.path == path) {
            return Self::cleanup_lease(claim);
        }
        let canonical = crate::worktree::canonical_planned_path(path);
        if canonical != path {
            return None;
        }
        let overlapping: Vec<_> = active
            .into_iter()
            .filter(|claim| paths_overlap(&canonical, &claim.path))
            .collect();
        if overlapping.is_empty() {
            DirectoryIdentity::read(&canonical)?;
            let claim = new_claim(canonical, DELETING);
            entries.push(Arc::downgrade(&claim));
            return Some(SourceDirectoryLease(claim));
        }
        None
    }

    fn cleanup_lease(claim: &Arc<DirectoryClaim>) -> Option<SourceDirectoryLease> {
        let lease = SourceDirectoryLease(Arc::clone(claim));
        if claim.phase.load(Ordering::SeqCst) != PREPARED || !lease.matches_directory() {
            return None;
        }
        claim.phase.store(DELETING, Ordering::SeqCst);
        Some(lease)
    }
}

impl SourceDirectoryLease {
    pub(super) fn path(&self) -> &Path {
        &self.0.path
    }
    pub(in crate::app) fn hold_writing(&self) {
        self.0.phase.store(WRITING, Ordering::SeqCst);
    }
    pub(in crate::app) fn mark_prepared(&self) {
        *self.0.identity.lock().unwrap() = DirectoryIdentity::read(&self.0.path);
        self.0.phase.store(PREPARED, Ordering::SeqCst);
    }

    pub(super) fn matches_directory(&self) -> bool {
        self.0
            .identity
            .lock()
            .unwrap()
            .is_some_and(|identity| DirectoryIdentity::read(&self.0.path) == Some(identity))
    }
}

impl AppState {
    pub(in crate::app) fn refuse_source_directory_claim(&self, path: &Path) -> Result<(), String> {
        // Callers pass their resolved source candidate. Reject only paths
        // overlapping the immutable deletion scope, without a global exclusion
        // or a filesystem probe under the app lock.
        for plan in &self.project_deletion_fences {
            if plan
                .roots
                .iter()
                .chain(plan.checkouts.iter().map(|c| &c.path))
                .any(|root| paths_overlap(path, root))
            {
                return self.refuse_deleting_identity(&plan.project_id, &plan.project_key);
            }
        }
        self.source_directory_claims.refuse_reference(path)
    }
}
