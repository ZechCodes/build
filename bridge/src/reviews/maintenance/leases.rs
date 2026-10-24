use super::*;
use crate::reviews::receivers::locks::RecoveryWait;
use crate::reviews::sync::reconcile::recovery::CaptureJournal;
use std::fs::File;

/// Explicit unlock matters: a child may inherit an otherwise live descriptor.
pub(super) struct Lease(File);
impl Lease {
    pub(super) fn acquire(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err("busy: receiver maintenance".into())
            }
            Err(error) => return Err(format!("receiver maintenance lease failed: {error}")),
        }
        Ok(Self(file))
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub(super) struct ReviewLeases {
    _capture: CaptureJournal,
    _merge: crate::reviews::merge::fence::MergeLease,
}

pub(super) fn review(candidate_pending: bool, review: &Review) -> Result<ReviewLeases, String> {
    let bindings: Vec<_> = review
        .bindings
        .iter()
        .filter(|binding| binding.receiving_repository.exists())
        .cloned()
        .collect();
    let capture = CaptureJournal::acquire_with_recovery_wait(
        &review.task_id,
        &bindings,
        RecoveryWait::TryOnly,
    )?;
    if candidate_pending {
        return Err("pending review capture must recover before reclamation".into());
    }
    let mut remaining = review.clone();
    remaining.bindings = bindings;
    let merge =
        crate::reviews::merge::fence::lease_with_recovery_wait(&remaining, RecoveryWait::TryOnly)?;
    Ok(ReviewLeases {
        _capture: capture,
        _merge: merge,
    })
}
