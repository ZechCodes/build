//! Only an opened repository can prove a ref absent. Unavailable repositories
//! leave durable debt; they never authorize clearing a receipt.
use super::*;

pub(crate) fn release(plans: &[HistoryPins]) -> Result<(), String> {
    if release_available(plans)? {
        return Ok(());
    }
    Err("review pin repository unavailable; receipt retained for startup retry".into())
}

/// Project deletion may complete with unavailable repositories still owed.
/// Other release errors (changed refs/markers, locks) retain the retry contract.
pub(crate) fn release_available(plans: &[HistoryPins]) -> Result<bool, String> {
    let mut drained = true;
    for plan in plans {
        let available = release_refs(&plan.refs)?;
        if available {
            release_markers(&plan.markers)?;
        }
        drained &= available;
        for snapshot in &plan.deferred_snapshots {
            drained &= release_snapshot(snapshot)?;
        }
    }
    Ok(drained)
}
fn release_refs(pins: &[PinRef]) -> Result<bool, String> {
    let mut drained = true;
    for pin in pins {
        match git2::Repository::open_bare(&pin.repository) {
            Ok(repository) => crate::reviews::capture::delete_pin_if_expected(
                &repository,
                &pin.name,
                Some(&pin.expected),
            )?,
            Err(_) => drained = false,
        }
    }
    Ok(drained)
}
fn release_markers(markers: &[PinMarker]) -> Result<(), String> {
    for marker in markers {
        match std::fs::read(&marker.path) {
            Ok(bytes) if bytes == marker.bytes => {
                std::fs::remove_file(&marker.path).map_err(|e| e.to_string())?
            }
            Ok(_) => return Err("review snapshot ownership marker changed".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
fn release_snapshot(snapshot: &DeferredSnapshotPins) -> Result<bool, String> {
    let repository = snapshot
        .directory
        .common_git_dir
        .as_ref()
        .ok_or("deferred pin receipt has no repository")?;
    match crate::reviews::publication::plan_snapshot_target_cleanup(
        &snapshot.task_id,
        &snapshot.snapshot_id,
        &snapshot.directory,
    ) {
        Ok(Some((target, marker))) => {
            let drained = release_refs(&[target])?;
            if drained {
                release_markers(&[marker])?;
            }
            Ok(drained)
        }
        Ok(None) => Ok(true), // The opened repository also proved target absent.
        Err(_) if git2::Repository::open_bare(repository).is_err() => Ok(false),
        Err(error) => Err(error),
    }
}
