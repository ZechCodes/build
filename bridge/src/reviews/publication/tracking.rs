//! Owned working-repository upstream refs for local review publication.

use super::super::model::ReviewBranchBinding;
use super::super::receivers::{refs, write_owned_json};
use super::remote::{short_branch, validate_tracking_alias};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TrackingClaim {
    common_git_dir: PathBuf,
    device: u64,
    inode: u64,
    repository_id: String,
    directory_id: String,
    source_id: String,
    remote_name: String,
    dedicated_branch_ref: String,
    receiving_repository: PathBuf,
    receiving_ref: String,
    tracking_ref: String,
    initial_head: String,
}

impl TrackingClaim {
    fn for_binding(
        repository: &git2::Repository,
        binding: &ReviewBranchBinding,
    ) -> Result<Self, String> {
        let common_git_dir = repository
            .commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let (device, inode) = directory_identity(&common_git_dir)?;
        let tracking_ref = format!(
            "refs/remotes/{}/{}",
            binding.remote_name,
            short_branch(binding)?
        );
        if !git2::Reference::is_valid_name(&tracking_ref) {
            return Err("invalid review upstream tracking ref".into());
        }
        Ok(Self {
            common_git_dir,
            device,
            inode,
            repository_id: binding.repository_id.clone(),
            directory_id: binding.directory_id.clone(),
            source_id: binding.source_id.clone(),
            remote_name: binding.remote_name.clone(),
            dedicated_branch_ref: binding.dedicated_branch_ref.clone(),
            receiving_repository: binding.receiving_repository.clone(),
            receiving_ref: binding.receiving_ref.clone(),
            tracking_ref,
            initial_head: binding.initial_head.clone(),
        })
    }

    fn path(&self) -> PathBuf {
        let key = format!(
            "{}\0{}\0{}",
            self.repository_id, self.directory_id, self.tracking_ref
        );
        self.common_git_dir
            .join("build-review-tracking")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
    }
}

pub(super) fn publish(binding: &ReviewBranchBinding) -> Result<(), String> {
    let repository =
        git2::Repository::open(&binding.working_repository).map_err(|error| error.to_string())?;
    let claim = TrackingClaim::for_binding(&repository, binding)?;
    refs::create_expected_reference_checked(
        &repository,
        &claim.tracking_ref,
        git2::Oid::from_str(&claim.initial_head).map_err(|error| error.to_string())?,
        || {
            validate_tracking_alias(binding, true)?;
            if !read_claim(&claim)? {
                if reference_exists(&repository, &claim.tracking_ref)? {
                    return Err(
                        "review tracking ref already exists without an ownership claim".into(),
                    );
                }
                ensure_claim_directory(&claim.path())?;
                write_owned_json(&claim.path(), &claim)?;
            }
            require_claim(&claim)?;
            checkpoint(binding, "tracking-locked");
            Ok(())
        },
    )
}

pub(super) fn validate_cleanup(binding: &ReviewBranchBinding) -> Result<(), String> {
    let Some(repository) = working_repository(binding)? else {
        return Ok(());
    };
    let claim = TrackingClaim::for_binding(&repository, binding)?;
    if !read_claim(&claim)? {
        return Ok(());
    }
    refs::validate_expected_reference_checked(
        &repository,
        &claim.tracking_ref,
        git2::Oid::from_str(&claim.initial_head).map_err(|error| error.to_string())?,
        || {
            require_claim(&claim)?;
            validate_present_alias(&repository, &claim, binding)
        },
    )
}

pub(super) fn cleanup(binding: &ReviewBranchBinding) -> Result<(), String> {
    let Some(repository) = working_repository(binding)? else {
        return Ok(());
    };
    let claim = TrackingClaim::for_binding(&repository, binding)?;
    if !read_claim(&claim)? {
        return Ok(());
    }
    refs::remove_expected_reference_finalized(
        &repository,
        &claim.tracking_ref,
        git2::Oid::from_str(&claim.initial_head).map_err(|error| error.to_string())?,
        || {
            // Ref deletion may have committed before a crash and alias cleanup.
            // With no ref left, release the exact durable claim even if the
            // already-removed alias no longer has its configuration entries.
            require_claim(&claim)?;
            validate_present_alias(&repository, &claim, binding)?;
            checkpoint(binding, "cleanup-locked");
            Ok(())
        },
        || {
            checkpoint(binding, "tracking-removed");
            require_claim(&claim)?;
            fs::remove_file(claim.path()).map_err(|error| error.to_string())?;
            sync_directory(
                claim
                    .path()
                    .parent()
                    .ok_or("invalid review tracking claim path")?,
            )
        },
    )
}

fn validate_present_alias(
    repository: &git2::Repository,
    claim: &TrackingClaim,
    binding: &ReviewBranchBinding,
) -> Result<(), String> {
    validate_tracking_alias(binding, reference_exists(repository, &claim.tracking_ref)?)
}

fn working_repository(binding: &ReviewBranchBinding) -> Result<Option<git2::Repository>, String> {
    match fs::symlink_metadata(&binding.working_repository) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(_) => git2::Repository::open(&binding.working_repository)
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

fn reference_exists(repository: &git2::Repository, name: &str) -> Result<bool, String> {
    match repository.find_reference(name) {
        Ok(_) => Ok(true),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn read_claim(expected: &TrackingClaim) -> Result<bool, String> {
    if directory_identity(&expected.common_git_dir)? != (expected.device, expected.inode) {
        return Err("review tracking repository directory identity changed".into());
    }
    let path = expected.path();
    validate_claim_directory(&path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err("review tracking ownership marker is not a regular file".into()),
    }
    let saved: TrackingClaim =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if saved != *expected {
        return Err("review tracking ownership marker changed".into());
    }
    Ok(true)
}

fn require_claim(expected: &TrackingClaim) -> Result<(), String> {
    if read_claim(expected)? {
        Ok(())
    } else {
        Err("review tracking ownership marker disappeared".into())
    }
}

fn validate_claim_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid review tracking claim path")?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err("review tracking ownership directory changed".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn ensure_claim_directory(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("invalid review tracking claim path")?;
    match fs::create_dir(parent) {
        Ok(()) => sync_directory(
            parent
                .parent()
                .ok_or("invalid review tracking claim root")?,
        )?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    validate_claim_directory(path)
}

fn directory_identity(directory: &Path) -> Result<(u64, u64), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_dir() {
        return Err("review tracking repository directory changed".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Err(
            "review tracking ownership requires verifiable directory identity on this platform"
                .into(),
        )
    }
}

fn sync_directory(directory: &Path) -> Result<(), String> {
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

pub(super) fn checkpoint(binding: &ReviewBranchBinding, phase: &str) {
    #[cfg(test)]
    if std::env::var("BUILD_REVIEW_INTERRUPTED_TRACKING_PHASE").as_deref() == Ok(phase) {
        let path = std::env::var_os("BUILD_REVIEW_INTERRUPTED_TRACKING_BINDING");
        let saved = path
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<ReviewBranchBinding>(&bytes).ok());
        if saved.is_some_and(|saved| {
            saved.working_repository == binding.working_repository
                && saved.dedicated_branch_ref == binding.dedicated_branch_ref
        }) {
            std::process::exit(27);
        }
    }
    #[cfg(not(test))]
    let _ = (binding, phase);
}
