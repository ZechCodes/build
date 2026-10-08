//! Literal review refs with registered, recoverable Git file locks.

use super::locks::{acquire_git_lock, OwnedGitFileLock};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Only the holder of this repository's packed-refs lock can reuse its lease.
pub(crate) struct PackedReferenceLease<'a> {
    directory: PathBuf,
    guard: &'a OwnedGitFileLock,
}

impl<'a> PackedReferenceLease<'a> {
    fn new(repository: &git2::Repository, guard: &'a OwnedGitFileLock) -> Result<Self, String> {
        Ok(Self {
            directory: repository
                .commondir()
                .canonicalize()
                .map_err(|error| error.to_string())?,
            guard,
        })
    }

    fn verify(&self, repository: &git2::Repository) -> Result<(), String> {
        let directory = repository
            .commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if self.directory != directory {
            return Err("review packed-reference lease belongs to another repository".into());
        }
        self.guard.verify_owned()
    }

    /// Settle an owned recovery expectation, also accepting an already-settled
    /// tip. The actual ref is read only after acquiring its native Git lock.
    pub(crate) fn settle_expected_reference_checked(
        &self,
        repository: &git2::Repository,
        reference: &str,
        expected: Option<git2::Oid>,
        new: git2::Oid,
        publish: impl FnOnce(&mut dyn FnMut() -> Result<(), String>) -> Result<(), String>,
    ) -> Result<bool, String> {
        self.update_checked(repository, reference, expected, new, true, publish)
    }

    /// Validate an exact tip under its native lock without publishing a ref.
    pub(crate) fn validate_expected_reference_checked(
        &self,
        repository: &git2::Repository,
        reference: &str,
        expected: Option<git2::Oid>,
        checkpoint: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.compare_expected_reference_checked(repository, reference, expected, checkpoint)? {
            return Err(format!("stale: review tracking lease changed: {reference}"));
        }
        Ok(())
    }

    /// Distinguish a locked readable mismatch from an unavailable comparison.
    pub(crate) fn compare_expected_reference_checked(
        &self,
        repository: &git2::Repository,
        reference: &str,
        expected: Option<git2::Oid>,
        checkpoint: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        validate_reference(reference)?;
        self.verify(repository)?;
        let directory = repository.commondir();
        let (guard, _) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
        guard.verify_owned()?;
        let current = reference_target(repository, reference)?;
        read_regular_optional(&directory.join(reference))?;
        checkpoint()?;
        guard.verify_owned()?;
        self.verify(repository)?;
        Ok(current == expected)
    }

    fn update_checked(
        &self,
        repository: &git2::Repository,
        reference: &str,
        expected: Option<git2::Oid>,
        new: git2::Oid,
        accept_settled: bool,
        publish: impl FnOnce(&mut dyn FnMut() -> Result<(), String>) -> Result<(), String>,
    ) -> Result<bool, String> {
        validate_reference(reference)?;
        self.verify(repository)?;
        let directory = repository.commondir();
        let (guard, mut file) =
            acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
        guard.verify_owned()?;
        let current = reference_target(repository, reference)?;
        if current != expected && !(accept_settled && current == Some(new)) {
            return Err(format!(
                "stale: review receiving lease changed: {reference}"
            ));
        }
        read_regular_optional(&directory.join(reference))?;
        repository
            .find_commit(new)
            .map_err(|error| error.to_string())?;
        let unchanged = current == Some(new);
        publish(&mut || {
            self.verify(repository)?;
            guard.verify_owned()?;
            if unchanged {
                return Ok(());
            }
            file.write_all(format!("{new}\n").as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            guard.publish_retaining_lock(&directory.join(reference))
        })?;
        Ok(!unchanged)
    }
}

/// Hold the exact working branch tip while an explicit publication imports and
/// publishes that immutable commit. Native Git writers honor these locks too.
pub(crate) fn with_expected_reference_locked<T>(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    locked: impl FnOnce(&PackedReferenceLease<'_>) -> Result<T, String>,
) -> Result<T, String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, _) = acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (guard, _) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    packed_guard.verify_owned()?;
    guard.verify_owned()?;
    if reference_target(repository, reference)? != Some(expected) {
        return Err(format!("stale: review working branch changed: {reference}"));
    }
    let lease = PackedReferenceLease::new(repository, &packed_guard)?;
    let result = locked(&lease);
    drop(guard);
    drop(packed_guard);
    result
}

/// Compare and replace one registered receiving ref without receive-pack's
/// unregistered locks. The metadata check runs after both Git locks are held.
pub(crate) fn update_expected_reference_checked(
    repository: &git2::Repository,
    reference: &str,
    expected: Option<git2::Oid>,
    new: git2::Oid,
    publish: impl FnOnce(&mut dyn FnMut() -> Result<(), String>) -> Result<(), String>,
) -> Result<bool, String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, _) = acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    PackedReferenceLease::new(repository, &packed_guard)?
        .update_checked(repository, reference, expected, new, false, publish)
}

pub(crate) fn create_expected_reference(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce(),
) -> Result<(), String> {
    create_expected_reference_checked(repository, reference, expected, || {
        checkpoint();
        Ok(())
    })
}

pub(crate) fn create_expected_reference_checked(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, _) = acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (guard, mut file) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    checkpoint()?;
    packed_guard.verify_owned()?;
    guard.verify_owned()?;
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
    remove_expected_reference_checked(repository, reference, expected, || {
        checkpoint();
        Ok(())
    })
}

pub(crate) fn remove_expected_reference_checked(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    remove_expected_reference_finalized(repository, reference, expected, checkpoint, || Ok(()))
}

/// Retain both Git locks through durable sidecar finalization after deletion.
pub(crate) fn remove_expected_reference_finalized(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce() -> Result<(), String>,
    finalize: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, mut packed_file) =
        acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (ref_guard, _) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    checkpoint()?;
    ref_guard.verify_owned()?;
    packed_guard.verify_owned()?;
    let Some(current) = reference_target(repository, reference)? else {
        return finalize();
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
    ref_guard.verify_owned()?;
    packed_guard.verify_owned()?;
    finalize()
}

/// Preflight under the same registered locks used by a later expected-OID
/// removal, including a packed entry hidden behind the loose reference.
pub(crate) fn validate_expected_reference_checked(
    repository: &git2::Repository,
    reference: &str,
    expected: git2::Oid,
    checkpoint: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    validate_reference(reference)?;
    let directory = repository.commondir();
    let (packed_guard, _) = acquire_git_lock(directory, Path::new("packed-refs.lock"))?;
    let (ref_guard, _) = acquire_git_lock(directory, Path::new(&format!("{reference}.lock")))?;
    checkpoint()?;
    packed_guard.verify_owned()?;
    ref_guard.verify_owned()?;
    if reference_target(repository, reference)?.is_some_and(|current| current != expected) {
        return Err(format!("review ref changed: {reference}"));
    }
    packed_without_reference(&directory.join("packed-refs"), reference, expected)?;
    read_regular_optional(&directory.join(reference))?;
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
