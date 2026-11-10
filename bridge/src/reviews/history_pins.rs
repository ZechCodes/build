//! Exact, serializable release receipts. Planning reads Git; only release mutates it.
use super::records::Review;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryPins {
    pub refs: Vec<PinRef>,
    pub markers: Vec<PinMarker>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinRef {
    pub repository: PathBuf,
    pub name: String,
    pub expected: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinMarker {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

pub(crate) fn plan(review: &Review) -> Result<HistoryPins, String> {
    let mut pins = HistoryPins::default();
    for snapshot in &review.snapshots {
        for directory in &snapshot.directories {
            let Some(repository) = &directory.common_git_dir else {
                continue;
            };
            let prefix = super::capture::pin_prefix(&review.task_id, &snapshot.id, &directory.id)?;
            for (suffix, expected) in [
                ("head", directory.head.as_deref()),
                (
                    "base",
                    directory.base.as_ref().map(|base| base.oid.as_str()),
                ),
            ] {
                if let Some(expected) = expected {
                    pins.refs.push(PinRef {
                        repository: repository.clone(),
                        name: format!("{prefix}/{suffix}"),
                        expected: expected.into(),
                    });
                }
            }
            if let Some((target, marker)) = super::publication::plan_snapshot_target_cleanup(
                &review.task_id,
                &snapshot.id,
                directory,
            )? {
                pins.refs.push(target);
                pins.markers.push(marker);
            }
        }
    }
    for pin in &pins.refs {
        validate(pin)?;
    }
    Ok(pins)
}
fn validate(pin: &PinRef) -> Result<(), String> {
    if !pin.repository.exists() {
        return Ok(());
    }
    let repository = git2::Repository::open_bare(&pin.repository).map_err(|e| e.to_string())?;
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
pub(crate) fn release(plans: &[HistoryPins]) -> Result<(), String> {
    for plan in plans {
        for pin in &plan.refs {
            if !pin.repository.exists() {
                continue;
            }
            let repository =
                git2::Repository::open_bare(&pin.repository).map_err(|e| e.to_string())?;
            super::capture::delete_pin_if_expected(&repository, &pin.name, Some(&pin.expected))?;
        }
        for marker in &plan.markers {
            match std::fs::read(&marker.path) {
                Ok(bytes) if bytes == marker.bytes => {
                    std::fs::remove_file(&marker.path).map_err(|e| e.to_string())?
                }
                Ok(_) => return Err("review snapshot ownership marker changed".into()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(())
}
