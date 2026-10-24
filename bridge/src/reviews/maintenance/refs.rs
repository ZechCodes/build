//! Terminal task refs are removed under the same native locks as publication.
use super::*;
use sha2::{Digest, Sha256};
use std::fs;

pub(super) fn cleanup(receiver: &Path, review: &Review, root: &Path) -> Result<bool, String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    let prefix = format!(
        "refs/build/reviews/{}/",
        crate::reviews::capture::encode_pin_component(&review.task_id)
    );
    let receiving: Vec<_> = review
        .bindings
        .iter()
        .filter(|binding| binding.receiving_repository == receiver)
        .map(|binding| binding.receiving_ref.as_str())
        .collect();
    let refs = repository
        .references()
        .map_err(|error| error.to_string())?
        .map(|reference| reference.map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut changed = false;
    for reference in refs {
        let name = reference.name().ok_or("review reference is not UTF-8")?;
        if !name.starts_with(&prefix) && !receiving.contains(&name) {
            continue;
        }
        let oid = reference
            .target()
            .ok_or("review reference became symbolic")?;
        crate::reviews::receivers::refs::remove_expected_reference_checked(
            &repository,
            name,
            oid,
            || Ok(()),
        )?;
        changed = true;
    }
    cleanup_markers(receiver, &review.task_id)?;
    let dispatch = root.join("operations").join(format!(
        "{:x}-dispatch.json",
        Sha256::digest(review.task_id.as_bytes())
    ));
    remove_optional(&dispatch)?;
    Ok(changed)
}

fn cleanup_markers(receiver: &Path, task: &str) -> Result<(), String> {
    let directory = receiver.join("build-review-snapshots");
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("snapshot marker placement changed".into());
        }
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(entry.path()).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if marker.get("task_id").and_then(|id| id.as_str()) == Some(task) {
            fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn remove_optional(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
