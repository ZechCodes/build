//! Literal review refs with registered, recoverable Git file locks.

use super::locks::acquire_git_lock;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

pub(crate) fn create_expected_reference(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce(),
) -> Result<(), String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (_packed_guard, _) = acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (guard, mut file) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    checkpoint();
    if reference_target(repository, reference)? == Some(expected) {
        return Ok(());
    }
    if reference_target(repository, reference)?.is_some() {
        return Err(format!("review pin changed: {reference}"));
    }
    repository
        .find_commit(expected)
        .map_err(|error| format!("review commit unavailable: {error}"))?;
    file.write_all(format!("{expected}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    guard.publish_retaining_lock(&directory.join(reference))
}

pub(crate) fn remove_expected_reference(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce(),
) -> Result<(), String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, mut packed_file) =
        acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (ref_guard, _) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    checkpoint();
    let Some(current) = reference_target(repository, reference)? else {
        return Ok(());
    };
    if current != expected {
        return Err(format!("review ref changed: {reference}"));
    }
    let packed_path = directory.join("packed-refs");
    let packed = packed_without_reference(&packed_path, reference, expected)?;
    let loose_path = directory.join(reference);
    ref_guard.verify_owned()?;
    packed_guard.verify_owned()?;
    if read_regular_optional(&loose_path)?.is_some() {
        fs::remove_file(&loose_path).map_err(|error| error.to_string())?;
        sync_parent(&loose_path)?;
    }
    if let Some(bytes) = packed {
        packed_file
            .write_all(&bytes)
            .and_then(|()| packed_file.sync_all())
            .map_err(|error| error.to_string())?;
        packed_guard.publish_retaining_lock(&packed_path)?;
    }
    Ok(())
}

fn validate_reference(reference: &str) -> Result<(), String> {
    if reference.starts_with("refs/") && git2::Reference::is_valid_name(reference) {
        Ok(())
    } else {
        Err("invalid review reference".into())
    }
}

fn reference_target(
    repository: &git2::Repository,
    reference: &str,
) -> Result<Option<git2::Oid>, String> {
    match repository.find_reference(reference) {
        Ok(current) => current
            .target()
            .map(Some)
            .ok_or_else(|| format!("review ref changed: {reference}")),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn sync_parent(path: &Path) -> Result<(), String> {
    File::open(path.parent().ok_or("invalid review ref path")?)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

fn read_regular_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::read(path).map(Some).map_err(|error| error.to_string())
        }
        Ok(_) => Err("review Git metadata is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Remove only the selected exact-OID entry, preserving all other packed bytes.
/// A changed entry hidden behind a loose ref must also block deletion.
pub(crate) fn packed_without_reference(
    path: &Path,
    reference: &str,
    expected: git2::Oid,
) -> Result<Option<Vec<u8>>, String> {
    let Some(bytes) = read_regular_optional(path)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let mut kept = Vec::with_capacity(bytes.len());
    let mut removed = false;
    let mut removing_peeled = false;
    for line in text.split_inclusive('\n') {
        if removing_peeled && line.starts_with('^') {
            removing_peeled = false;
            continue;
        }
        removing_peeled = false;
        if packed_line_matches(line, reference, expected)? {
            if removed {
                return Err("review packed ref has duplicate entries".into());
            }
            removed = true;
            removing_peeled = true;
        } else {
            kept.extend_from_slice(line.as_bytes());
        }
    }
    Ok(removed.then_some(kept))
}

fn packed_line_matches(line: &str, reference: &str, expected: git2::Oid) -> Result<bool, String> {
    let Some((oid, name)) = line.trim_end().split_once(' ') else {
        return Ok(false);
    };
    if name != reference {
        return Ok(false);
    }
    if git2::Oid::from_str(oid).map_err(|error| error.to_string())? != expected {
        return Err(format!("review packed ref changed: {reference}"));
    }
    Ok(true)
}
