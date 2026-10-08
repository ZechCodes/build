//! Immutable receiver snapshot pin plans and publication guards.

use super::super::model::{
    ReviewBase, ReviewBaseKind, ReviewBranchBinding, ReviewDirectory, ReviewDirectoryStatus,
    ReviewMembership, ReviewMembershipKind, ReviewSnapshot,
};
use super::super::receivers::{
    validate_binding_receiver, validate_recorded_receiver, write_owned_json,
};
use super::remote::short_branch;
use super::{comparison_base, import_commit, remove_expected_ref, source_base};
use crate::tracker::Actor;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub fn capture_snapshot(
    task_id: &str,
    snapshot_id: &str,
    workspace: &Workspace,
    bindings: &[ReviewBranchBinding],
    memberships: &[ReviewMembership],
    author: &Actor,
) -> Result<ReviewSnapshot, String> {
    let mut snapshot = ReviewSnapshot {
        id: snapshot_id.into(),
        number: 0,
        created_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|error| error.to_string())?,
        author: author.clone(),
        directories: Vec::with_capacity(workspace.directories.len()),
    };
    for directory in &workspace.directories {
        let membership = memberships
            .iter()
            .find(|member| member.directory_id == directory.id)
            .ok_or_else(|| format!("missing review membership: {}", directory.id))?;
        let mut saved = ReviewDirectory::from(directory);
        match membership.kind {
            ReviewMembershipKind::Git => {
                let binding = bindings
                    .iter()
                    .find(|binding| binding.directory_id == directory.id)
                    .ok_or_else(|| format!("missing review branch binding: {}", directory.id))?;
                capture_git(task_id, snapshot_id, binding, &mut saved)?;
            }
            ReviewMembershipKind::Live => {
                saved.status = ReviewDirectoryStatus::NotGit;
                saved.is_git = false;
                saved.reason = membership.reason.clone();
            }
            ReviewMembershipKind::Excluded => {
                saved.status = ReviewDirectoryStatus::Unavailable;
                saved.reason = membership
                    .reason
                    .clone()
                    .or_else(|| Some("Excluded from review".into()));
            }
        }
        snapshot.directories.push(saved);
    }
    Ok(snapshot)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SnapshotPins {
    task_id: String,
    snapshot_id: String,
    directory_id: String,
    head: String,
    pub(super) source_base: String,
    comparison_base: String,
}

pub(super) fn snapshot_marker(
    binding: &ReviewBranchBinding,
    task_id: &str,
    snapshot_id: &str,
) -> PathBuf {
    snapshot_marker_at(
        &binding.receiving_repository,
        &binding.directory_id,
        task_id,
        snapshot_id,
    )
}

fn snapshot_marker_at(
    receiver: &Path,
    directory_id: &str,
    task_id: &str,
    snapshot_id: &str,
) -> PathBuf {
    let identity = format!("{task_id}\0{snapshot_id}\0{directory_id}");
    receiver
        .join("build-review-snapshots")
        .join(format!("{:x}.json", Sha256::digest(identity.as_bytes())))
}

fn read_snapshot_pins(
    binding: &ReviewBranchBinding,
    task_id: &str,
    snapshot_id: &str,
) -> Result<Option<SnapshotPins>, String> {
    read_saved_pins(
        &binding.receiving_repository,
        &binding.directory_id,
        task_id,
        snapshot_id,
    )
}

fn read_saved_pins(
    receiver: &Path,
    directory_id: &str,
    task_id: &str,
    snapshot_id: &str,
) -> Result<Option<SnapshotPins>, String> {
    let path = snapshot_marker_at(receiver, directory_id, task_id, snapshot_id);
    validate_snapshot_marker_root(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() {
        return Err("review snapshot ownership marker changed".into());
    }
    let pins: SnapshotPins =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if pins.task_id != task_id
        || pins.snapshot_id != snapshot_id
        || pins.directory_id != directory_id
    {
        return Err("review snapshot ownership marker changed".into());
    }
    Ok(Some(pins))
}

fn validate_snapshot_marker_root(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid snapshot marker path")?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err("review snapshot ownership directory changed".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn plan_snapshot_pins(
    binding: &ReviewBranchBinding,
    task_id: &str,
    snapshot_id: &str,
) -> Result<SnapshotPins, String> {
    if let Some(pins) = read_snapshot_pins(binding, task_id, snapshot_id)? {
        return Ok(pins);
    }
    let head = binding
        .last_received_head
        .as_ref()
        .unwrap_or(&binding.initial_head)
        .clone();
    let source_base = source_base(binding)?;
    import_commit(
        &binding.receiving_repository,
        &binding.source_repository,
        &source_base,
    )?;
    let comparison_base = comparison_base(binding, &head, &source_base)?;
    let pins = SnapshotPins {
        task_id: task_id.into(),
        snapshot_id: snapshot_id.into(),
        directory_id: binding.directory_id.clone(),
        head,
        source_base,
        comparison_base,
    };
    let path = snapshot_marker(binding, task_id, snapshot_id);
    validate_snapshot_marker_root(&path)?;
    fs::create_dir_all(path.parent().ok_or("invalid snapshot marker path")?)
        .map_err(|error| error.to_string())?;
    write_owned_json(&path, &pins)?;
    read_snapshot_pins(binding, task_id, snapshot_id)?
        .ok_or_else(|| "review snapshot ownership marker is missing".into())
}

fn capture_git(
    task_id: &str,
    snapshot_id: &str,
    binding: &ReviewBranchBinding,
    saved: &mut ReviewDirectory,
) -> Result<(), String> {
    validate_binding_receiver(binding)?;
    let pins = plan_snapshot_pins(binding, task_id, snapshot_id)?;
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| error.to_string())?;
    let prefix = super::super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
    for (name, oid) in [
        ("head", &pins.head),
        ("base", &pins.comparison_base),
        ("target", &pins.source_base),
    ] {
        create_expected_pin(&repository, &format!("{prefix}/{name}"), oid)?;
    }
    let working =
        git2::Repository::open(&binding.working_repository).map_err(|error| error.to_string())?;
    saved.common_git_dir = Some(binding.receiving_repository.clone());
    saved.status = ReviewDirectoryStatus::Git;
    saved.reason = None;
    saved.branch = Some(short_branch(binding)?.into());
    saved.head = Some(pins.head);
    saved.base = Some(ReviewBase {
        kind: ReviewBaseKind::Configured,
        name: Some(
            binding
                .base_branch_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&binding.base_branch_ref)
                .into(),
        ),
        oid: pins.comparison_base,
    });
    saved.uncommitted_files = Some(super::super::capture::count_uncommitted(&working)?);
    Ok(())
}

pub(super) fn create_expected_pin(
    repository: &git2::Repository,
    name: &str,
    expected: &str,
) -> Result<(), String> {
    super::super::receivers::refs::create_expected_reference(
        repository,
        name,
        git2::Oid::from_str(expected).map_err(|error| error.to_string())?,
        || pin_mutation_checkpoint(repository, "create"),
    )
}

pub(super) fn pin_mutation_checkpoint(repository: &git2::Repository, operation: &str) {
    #[cfg(test)]
    if std::env::var_os("BUILD_REVIEW_INTERRUPTED_PIN_DIR").as_deref()
        == Some(repository.path().as_os_str())
        && std::env::var("BUILD_REVIEW_INTERRUPTED_PIN_OPERATION").as_deref() == Ok(operation)
    {
        std::process::exit(25);
    }
    #[cfg(not(test))]
    let _ = (repository, operation);
}

/// Cancel only refs whose expected OIDs were saved before pin creation.
pub fn cleanup_opening_pins(
    task_id: &str,
    snapshot_id: &str,
    bindings: &[ReviewBranchBinding],
) -> Result<(), String> {
    for binding in bindings {
        let Some(pins) = read_snapshot_pins(binding, task_id, snapshot_id)? else {
            continue;
        };
        validate_binding_receiver(binding)?;
        let repository = git2::Repository::open_bare(&binding.receiving_repository)
            .map_err(|error| error.to_string())?;
        let prefix =
            super::super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
        for (name, oid) in [
            ("head", &pins.head),
            ("base", &pins.comparison_base),
            ("target", &pins.source_base),
        ] {
            remove_expected_ref(&repository, &format!("{prefix}/{name}"), oid)?;
        }
        fs::remove_file(snapshot_marker(binding, task_id, snapshot_id))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Release the source-base tip retained for a published receiver snapshot.
pub fn cleanup_snapshot_target(
    task_id: &str,
    snapshot_id: &str,
    directory: &ReviewDirectory,
) -> Result<(), String> {
    let Some(receiver) = directory.common_git_dir.as_deref() else {
        return Ok(());
    };
    let Some(pins) = read_saved_pins(receiver, &directory.id, task_id, snapshot_id)? else {
        return Ok(());
    };
    validate_recorded_receiver(receiver)?;
    if directory.head.as_deref() != Some(pins.head.as_str())
        || directory.base.as_ref().map(|base| base.oid.as_str())
            != Some(pins.comparison_base.as_str())
    {
        return Err("review snapshot ownership metadata changed".into());
    }
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    let prefix = super::super::capture::pin_prefix(task_id, snapshot_id, &directory.id)?;
    remove_expected_ref(&repository, &format!("{prefix}/target"), &pins.source_base)?;
    fs::remove_file(snapshot_marker_at(
        receiver,
        &directory.id,
        task_id,
        snapshot_id,
    ))
    .map_err(|error| error.to_string())
}

/// Recheck the immutable opening's received head and every owned snapshot pin.
pub fn validate_snapshot_pins(
    task_id: &str,
    snapshot_id: &str,
    bindings: &[ReviewBranchBinding],
) -> Result<(), String> {
    for binding in bindings {
        validate_binding_receiver(binding)?;
        let repository = git2::Repository::open_bare(&binding.receiving_repository)
            .map_err(|error| error.to_string())?;
        validate_initial_pins(&repository, task_id, snapshot_id, binding)?;
    }
    Ok(())
}

fn initial_pin_refs(
    task_id: &str,
    snapshot_id: &str,
    binding: &ReviewBranchBinding,
) -> Result<Vec<(String, String)>, String> {
    let pins = read_snapshot_pins(binding, task_id, snapshot_id)?
        .ok_or("review snapshot ownership marker is missing")?;
    if pins.head != binding.initial_head {
        return Err("review snapshot head changed from the opening commit".into());
    }
    let prefix = super::super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
    Ok(vec![
        (binding.receiving_ref.clone(), binding.initial_head.clone()),
        (format!("{prefix}/head"), pins.head),
        (format!("{prefix}/base"), pins.comparison_base),
        (format!("{prefix}/target"), pins.source_base),
    ])
}

fn validate_initial_pins(
    repository: &git2::Repository,
    task_id: &str,
    snapshot_id: &str,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    for (name, expected) in initial_pin_refs(task_id, snapshot_id, binding)? {
        let actual = repository
            .find_reference(&name)
            .map_err(|error| error.to_string())?;
        if actual.target().map(|oid| oid.to_string()).as_deref() != Some(&expected) {
            return Err(format!(
                "review ref changed before opening publication: {name}"
            ));
        }
    }
    Ok(())
}

/// Hold every receiving ref and immutable snapshot pin across the metadata
/// transaction. A terminal push cannot advance the first snapshot's head
/// between its final Git validation and publication in the store.
pub fn with_initial_receivers_locked<T>(
    bindings: &[ReviewBranchBinding],
    task_id: &str,
    snapshot_id: &str,
    publish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut grouped: BTreeMap<PathBuf, Vec<&ReviewBranchBinding>> = BTreeMap::new();
    for binding in bindings {
        validate_binding_receiver(binding)?;
        grouped
            .entry(binding.receiving_repository.clone())
            .or_default()
            .push(binding);
    }
    let groups: Vec<_> = grouped.into_iter().collect();
    lock_receivers(&groups, task_id, snapshot_id, publish)
}

fn lock_receivers<T>(
    groups: &[(PathBuf, Vec<&ReviewBranchBinding>)],
    task_id: &str,
    snapshot_id: &str,
    publish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let Some((path, bindings)) = groups.first() else {
        return publish();
    };
    let repository = git2::Repository::open_bare(path).map_err(|error| error.to_string())?;
    let mut refs = BTreeSet::new();
    for binding in bindings {
        refs.extend(
            initial_pin_refs(task_id, snapshot_id, binding)?
                .into_iter()
                .map(|(name, _)| name),
        );
    }
    let mut locks = Vec::with_capacity(refs.len());
    for name in refs {
        let (lock, _) = super::super::receivers::locks::acquire_git_lock(
            path,
            Path::new(&format!("{name}.lock")),
        )?;
        locks.push(lock);
    }
    for binding in bindings {
        validate_initial_pins(&repository, task_id, snapshot_id, binding)?;
    }
    let result = lock_receivers(&groups[1..], task_id, snapshot_id, publish);
    drop(locks);
    result
}
