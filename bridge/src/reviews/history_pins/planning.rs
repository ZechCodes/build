//! Capture known refs even when Git is offline. Capture the exact snapshot
//! namespace too when its target ownership metadata cannot yet be read.
use super::*;
use crate::reviews::{model::ReviewDirectory, records::Review};

pub(crate) fn plan(review: &Review) -> Result<HistoryPins, String> {
    let mut pins = HistoryPins::default();
    for snapshot in &review.snapshots {
        for directory in &snapshot.directories {
            append_directory(&mut pins, &review.task_id, &snapshot.id, directory)?;
        }
    }
    for pin in &pins.refs {
        validate_available_pin(pin)?;
    }
    Ok(pins)
}
fn append_directory(
    pins: &mut HistoryPins,
    task: &str,
    snapshot: &str,
    directory: &ReviewDirectory,
) -> Result<(), String> {
    let Some(repository) = &directory.common_git_dir else {
        return Ok(());
    };
    let prefix = crate::reviews::capture::pin_prefix(task, snapshot, &directory.id)?;
    for (suffix, expected) in [
        ("head", directory.head.as_deref()),
        ("base", directory.base.as_ref().map(|b| b.oid.as_str())),
    ] {
        if let Some(expected) = expected {
            pins.refs.push(PinRef {
                repository: repository.clone(),
                name: format!("{prefix}/{suffix}"),
                expected: expected.into(),
            });
        }
    }
    match crate::reviews::publication::plan_snapshot_target_cleanup(task, snapshot, directory) {
        Ok(Some((target, marker))) => {
            pins.refs.push(target);
            pins.markers.push(marker);
        }
        Ok(None) => {}
        Err(_) if git2::Repository::open_bare(repository).is_err() => {
            pins.deferred_snapshots.push(DeferredSnapshotPins {
                task_id: task.into(),
                snapshot_id: snapshot.into(),
                directory: directory.clone(),
            });
        }
        Err(error) => return Err(error),
    }
    Ok(())
}
fn validate_available_pin(pin: &PinRef) -> Result<(), String> {
    // This is preflight, not a drain. The known ref and deferred snapshot are
    // durably recorded even when Git cannot be opened; absence is not assumed.
    let Ok(repository) = git2::Repository::open_bare(&pin.repository) else {
        return Ok(());
    };
    let result = match repository.find_reference(&pin.name) {
        Ok(reference)
            if reference.target().map(|id| id.to_string()).as_deref() == Some(&pin.expected) =>
        {
            Ok(())
        }
        Ok(_) => Err(format!("review pin changed: {}", pin.name)),
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    };
    result
}
