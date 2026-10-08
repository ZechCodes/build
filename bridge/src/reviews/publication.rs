//! Branch-scoped publication into durable local review receivers.

use super::model::ReviewBranchBinding;
use super::receivers::validate_binding_receiver;
use crate::git_process::{git_failure, run_git_unattended};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::Duration;

mod pins;
mod received;
mod remote;
mod tracking;

use pins::pin_mutation_checkpoint;
pub use pins::{
    capture_received_directory, capture_snapshot, cleanup_opening_pins, cleanup_received_pins,
    cleanup_snapshot_target, validate_snapshot_pins, with_initial_receivers_locked,
    with_received_snapshot_locked,
};
pub use received::{observe_received, ReceivedCommit};
use remote::validate_push_destination;
pub use remote::{
    choose_remote_name, choose_remote_name_avoiding, cleanup_remote, configure_remote,
};

/// Check the original owned branch routing and effective push destination
/// without changing Git configuration or publication refs.
pub(crate) fn validate_bound_remote(binding: &ReviewBranchBinding) -> Result<(), String> {
    remote::validate_tracking_alias(binding, true)?;
    remote::validate_push_destination(binding)
}

#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Interrupted(String),
}

pub fn publish_initial(binding: &ReviewBranchBinding) -> Result<String, PublicationError> {
    validate_binding_receiver(binding).map_err(PublicationError::Failed)?;
    validate_push_destination(binding).map_err(PublicationError::Failed)?;
    let received = received_head(binding).map_err(PublicationError::Failed)?;
    if received.as_deref() == Some(&binding.initial_head) {
        tracking::checkpoint(binding, "after-receiver");
        tracking::publish(binding).map_err(PublicationError::Failed)?;
        return Ok(binding.initial_head.clone());
    }
    if received.is_some() {
        return Err(PublicationError::Failed(
            "review receiving branch changed before initial publication".into(),
        ));
    }
    let reference = git2::Repository::open(&binding.working_repository)
        .and_then(|repository| repository.refname_to_id(&binding.dedicated_branch_ref))
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    if reference.to_string() != binding.initial_head {
        return Err(PublicationError::Failed(
            "review working branch changed before initial publication".into(),
        ));
    }
    receive_initial(binding)?;
    if received_head(binding)
        .map_err(PublicationError::Failed)?
        .as_deref()
        != Some(&binding.initial_head)
    {
        return Err(PublicationError::Failed(
            "initial review publication did not receive the expected commit".into(),
        ));
    }
    tracking::checkpoint(binding, "after-receiver");
    tracking::publish(binding).map_err(PublicationError::Failed)?;
    Ok(binding.initial_head.clone())
}

pub fn cleanup_initial(binding: &ReviewBranchBinding) -> Result<(), String> {
    validate_initial_cleanup(binding)?;
    tracking::cleanup(binding)?;
    match fs::symlink_metadata(&binding.receiving_repository) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    validate_binding_receiver(binding)?;
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| error.to_string())?;
    remove_expected_ref(&repository, &binding.receiving_ref, &binding.initial_head)
}

/// Preserve every owned setup claim when either publication ref has changed
/// or its exact Git lock is owned by another writer.
pub fn validate_initial_cleanup(binding: &ReviewBranchBinding) -> Result<(), String> {
    match fs::symlink_metadata(&binding.receiving_repository) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
        Ok(_) => {
            validate_binding_receiver(binding)?;
            let repository = git2::Repository::open_bare(&binding.receiving_repository)
                .map_err(|error| error.to_string())?;
            super::receivers::refs::validate_expected_reference_checked(
                &repository,
                &binding.receiving_ref,
                git2::Oid::from_str(&binding.initial_head).map_err(|error| error.to_string())?,
                || Ok(()),
            )?;
        }
    }
    tracking::validate_cleanup(binding)
}

pub fn validate_bases(bindings: &[ReviewBranchBinding]) -> Result<(), String> {
    for binding in bindings {
        validate_binding_receiver(binding)?;
        import_commit(
            &binding.receiving_repository,
            &binding.working_repository,
            &binding.initial_head,
        )?;
        let base = source_base(binding)?;
        import_commit(
            &binding.receiving_repository,
            &binding.source_repository,
            &base,
        )?;
        comparison_base(binding, &binding.initial_head, &base)?;
    }
    Ok(())
}

/// A health observation may still read the receiver after source removal.
pub fn registered_received_head(binding: &ReviewBranchBinding) -> Result<Option<String>, String> {
    super::receivers::validate_registered_receiver(binding)?;
    received_head(binding)
}

fn received_head(binding: &ReviewBranchBinding) -> Result<Option<String>, String> {
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| error.to_string())?;
    let received = match repository.find_reference(&binding.receiving_ref) {
        Ok(reference) => reference
            .target()
            .map(|oid| Some(oid.to_string()))
            .ok_or_else(|| "review receiving branch became symbolic".into()),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    };
    received
}

fn receive_initial(binding: &ReviewBranchBinding) -> Result<(), PublicationError> {
    // Fetch only objects: receive-pack's unregistered ref lock cannot be
    // recovered safely after death. Publish the exact OID with our own lock.
    import_publication_commit(
        &binding.receiving_repository,
        &binding.working_repository,
        &binding.initial_head,
    )?;
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    super::receivers::refs::create_expected_reference(
        &repository,
        &binding.receiving_ref,
        git2::Oid::from_str(&binding.initial_head)
            .map_err(|error| PublicationError::Failed(error.to_string()))?,
        || {
            #[cfg(test)]
            if std::env::var_os("BUILD_REVIEW_INTERRUPTED_INITIAL_RECEIVER").as_deref()
                == Some(binding.receiving_repository.as_os_str())
            {
                std::process::exit(26);
            }
        },
    )
    .map_err(PublicationError::Failed)
}

fn source_base(binding: &ReviewBranchBinding) -> Result<String, String> {
    let repository =
        git2::Repository::open(&binding.source_repository).map_err(|error| error.to_string())?;
    repository
        .find_reference(&binding.base_branch_ref)
        .and_then(|reference| reference.peel_to_commit())
        .map(|commit| commit.id().to_string())
        .map_err(|error| format!("source base branch unavailable: {error}"))
}

fn import_commit(receiver: &Path, source: &Path, oid: &str) -> Result<(), String> {
    import_publication_commit(receiver, source, oid).map_err(|error| error.to_string())
}

fn import_publication_commit(
    receiver: &Path,
    source: &Path,
    oid: &str,
) -> Result<(), PublicationError> {
    let oid =
        git2::Oid::from_str(oid).map_err(|error| PublicationError::Failed(error.to_string()))?;
    let source = source
        .canonicalize()
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    let source = source
        .to_str()
        .ok_or_else(|| PublicationError::Failed("review source path is not UTF-8".into()))?;
    if publication_git(receiver, &["ls-remote", "--get-url", "--", source])?.trim() != source {
        return Err(PublicationError::Failed(
            "local review source URL was rewritten to a different destination".into(),
        ));
    }
    publication_git(
        receiver,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "protocol.file.allow=always",
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--",
            source,
            &oid.to_string(),
        ],
    )?;
    let repository = git2::Repository::open_bare(receiver)
        .map_err(|error| PublicationError::Failed(error.to_string()))?;
    repository.find_commit(oid).map(|_| ()).map_err(|error| {
        PublicationError::Failed(format!("review commit unavailable after import: {error}"))
    })
}

fn publication_git(repository: &Path, args: &[&str]) -> Result<String, PublicationError> {
    let arguments: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let output =
        run_git_unattended(repository, &arguments, Duration::from_secs(30)).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::TimedOut => PublicationError::Interrupted(format!(
                    "local review publication was interrupted: {error}"
                )),
                _ => PublicationError::Failed(error.to_string()),
            }
        })?;
    if !output.status.success() {
        return Err(PublicationError::Failed(
            git_failure(&arguments, &output).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn comparison_base(
    binding: &ReviewBranchBinding,
    head: &str,
    source_base: &str,
) -> Result<String, String> {
    let repository = git2::Repository::open_bare(&binding.receiving_repository)
        .map_err(|error| error.to_string())?;
    let head = git2::Oid::from_str(head).map_err(|error| error.to_string())?;
    let base = git2::Oid::from_str(source_base).map_err(|error| error.to_string())?;
    repository
        .merge_base(head, base)
        .map(|oid| oid.to_string())
        .map_err(|error| {
            format!("review head and source base are unrelated or unavailable: {error}")
        })
}

fn remove_expected_ref(
    repository: &git2::Repository,
    reference: &str,
    expected: &str,
) -> Result<(), String> {
    super::receivers::refs::remove_expected_reference(
        repository,
        reference,
        git2::Oid::from_str(expected).map_err(|error| error.to_string())?,
        || pin_mutation_checkpoint(repository, "remove"),
    )
}

#[cfg(test)]
#[path = "publication/tests.rs"]
mod tests;
