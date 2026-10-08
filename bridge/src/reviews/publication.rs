//! Branch-scoped publication into durable local review receivers.

use super::model::{
    ReviewBase, ReviewBaseKind, ReviewBranchBinding, ReviewDirectory, ReviewDirectoryStatus,
    ReviewMembership, ReviewMembershipKind, ReviewSnapshot,
};
use super::receivers::{
    git, validate_binding_receiver, validate_recorded_receiver, with_local_config_locked,
    write_owned_json,
};
use crate::git_process::{git_failure, run_git_unattended};
use crate::tracker::Actor;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum PublicationError {
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Interrupted(String),
}

pub fn choose_remote_name(binding: &ReviewBranchBinding) -> Result<String, String> {
    choose_remote_name_avoiding(binding, &BTreeSet::new())
}

/// Reserve aliases while planning multiple directories that share Git config.
pub fn choose_remote_name_avoiding(
    binding: &ReviewBranchBinding,
    reserved: &BTreeSet<String>,
) -> Result<String, String> {
    let repository =
        git2::Repository::open(&binding.working_repository).map_err(|error| error.to_string())?;
    let config = repository.config().map_err(|error| error.to_string())?;
    let suffix = format!(
        "{:x}",
        Sha256::digest(binding.dedicated_branch_ref.as_bytes())
    );
    for index in 0..1000 {
        let alias = match index {
            0 => "build-review".into(),
            1 => format!("build-review-{}", &suffix[..12]),
            _ => format!("build-review-{}-{index}", &suffix[..12]),
        };
        if !reserved.contains(&alias)
            && (remote_keys(&config, &alias)?.is_empty()
                || remote_matches(&config, binding, &alias)?)
        {
            return Ok(alias);
        }
    }
    Err("could not allocate a local review remote alias".into())
}

pub fn configure_remote(binding: &ReviewBranchBinding) -> Result<(), String> {
    validate_binding_receiver(binding)?;
    let expected = configuration(binding)?;
    // Recheck and replace all owned entries while Git's local config is locked.
    with_local_config_locked(&binding.working_repository, |config, local| {
        validate_configuration(config, binding, &expected)?;
        for (key, value) in &expected {
            local
                .set_str(key, value)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })?;
    validate_push_destination(binding)
}

pub fn cleanup_remote(binding: &ReviewBranchBinding) -> Result<(), String> {
    let expected = configuration(binding)?;
    with_local_config_locked(&binding.working_repository, |config, local| {
        validate_configuration(config, binding, &expected)?;
        for (key, value) in expected.iter().rev() {
            if config_values(local, key)? == [value.as_str()] {
                local.remove(key).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })
}

pub fn publish_initial(binding: &ReviewBranchBinding) -> Result<String, PublicationError> {
    validate_binding_receiver(binding).map_err(PublicationError::Failed)?;
    validate_push_destination(binding).map_err(PublicationError::Failed)?;
    let received = received_head(binding).map_err(PublicationError::Failed)?;
    if received.as_deref() == Some(&binding.initial_head) {
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
    push_initial(binding)?;
    if received_head(binding)
        .map_err(PublicationError::Failed)?
        .as_deref()
        != Some(&binding.initial_head)
    {
        return Err(PublicationError::Failed(
            "initial review publication did not receive the expected commit".into(),
        ));
    }
    Ok(binding.initial_head.clone())
}

pub fn cleanup_initial(binding: &ReviewBranchBinding) -> Result<(), String> {
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

fn short_branch(binding: &ReviewBranchBinding) -> Result<&str, String> {
    binding
        .dedicated_branch_ref
        .strip_prefix("refs/heads/")
        .filter(|_| git2::Reference::is_valid_name(&binding.dedicated_branch_ref))
        .ok_or_else(|| "invalid dedicated review branch".into())
}

fn configuration(binding: &ReviewBranchBinding) -> Result<Vec<(String, String)>, String> {
    let branch = short_branch(binding)?;
    if !git2::Reference::is_valid_name(&binding.receiving_ref)
        || !binding.receiving_ref.starts_with("refs/heads/")
    {
        return Err("invalid review receiving branch".into());
    }
    if binding.remote_name.is_empty()
        || !binding
            .remote_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid local review remote alias".into());
    }
    let prefix = format!("remote.{}", binding.remote_name);
    let url = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    Ok(vec![
        (
            format!("{prefix}.buildreviewrepository"),
            binding.repository_id.clone(),
        ),
        (
            format!("{prefix}.buildreviewbranch"),
            binding.dedicated_branch_ref.clone(),
        ),
        (format!("{prefix}.url"), url.into()),
        (format!("{prefix}.pushurl"), url.into()),
        (
            format!("{prefix}.fetch"),
            format!(
                "+{}:refs/remotes/{}/{}",
                binding.receiving_ref, binding.remote_name, branch
            ),
        ),
        (
            format!("{prefix}.push"),
            format!("{}:{}", binding.dedicated_branch_ref, binding.receiving_ref),
        ),
        (
            format!("branch.{branch}.remote"),
            binding.remote_name.clone(),
        ),
        (
            format!("branch.{branch}.merge"),
            binding.receiving_ref.clone(),
        ),
        (
            format!("branch.{branch}.pushremote"),
            binding.remote_name.clone(),
        ),
    ])
}

fn config_values(config: &git2::Config, key: &str) -> Result<Vec<String>, String> {
    let mut entries = match config.multivar(key, None) {
        Ok(entries) => entries,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut values = Vec::new();
    while let Some(entry) = entries.next() {
        values.push(
            entry
                .map_err(|error| error.to_string())?
                .value()
                .ok_or("review Git configuration is not UTF-8")?
                .into(),
        );
    }
    Ok(values)
}

fn remote_keys(config: &git2::Config, alias: &str) -> Result<Vec<String>, String> {
    let prefix = format!("remote.{alias}.");
    let mut entries = config.entries(None).map_err(|error| error.to_string())?;
    let mut keys = Vec::new();
    while let Some(entry) = entries.next() {
        if let Some(name) = entry
            .map_err(|error| error.to_string())?
            .name()
            .filter(|name| name.starts_with(&prefix))
        {
            keys.push(name.to_owned());
        }
    }
    Ok(keys)
}

fn remote_matches(
    config: &git2::Config,
    binding: &ReviewBranchBinding,
    alias: &str,
) -> Result<bool, String> {
    let url = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    let prefix = format!("remote.{alias}");
    Ok(config_values(config, &format!("{prefix}.url"))? == [url]
        && config_values(config, &format!("{prefix}.pushurl"))? == [url]
        && config_values(config, &format!("{prefix}.buildreviewrepository"))?
            == [binding.repository_id.as_str()]
        && config_values(config, &format!("{prefix}.buildreviewbranch"))?
            == [binding.dedicated_branch_ref.as_str()])
}

fn validate_configuration(
    config: &git2::Config,
    binding: &ReviewBranchBinding,
    expected: &[(String, String)],
) -> Result<(), String> {
    let keys = remote_keys(config, &binding.remote_name)?;
    if !keys.is_empty()
        && config_values(
            config,
            &format!("remote.{}.buildreviewrepository", binding.remote_name),
        )? != [binding.repository_id.as_str()]
    {
        return Err("local review remote ownership changed".into());
    }
    if keys
        .iter()
        .any(|key| !expected.iter().any(|(owned, _)| owned == key))
    {
        return Err("local review remote configuration changed".into());
    }
    for (key, expected) in expected {
        let values = config_values(config, key)?;
        if !values.is_empty() && values != [expected.as_str()] {
            return Err(format!("local review Git configuration changed: {key}"));
        }
    }
    Ok(())
}

fn validate_push_destination(binding: &ReviewBranchBinding) -> Result<(), String> {
    let destinations = git(
        &binding.working_repository,
        &["remote", "get-url", "--push", "--all", &binding.remote_name],
    )?;
    let expected = binding
        .receiving_repository
        .to_str()
        .ok_or("review receiver path is not UTF-8")?;
    if destinations.lines().collect::<Vec<_>>() != [expected] {
        return Err("local review remote push destination changed".into());
    }
    Ok(())
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

fn push_initial(binding: &ReviewBranchBinding) -> Result<(), PublicationError> {
    let lease = format!("--force-with-lease={}:", binding.receiving_ref);
    let refspec = format!("{}:{}", binding.initial_head, binding.receiving_ref);
    let args = [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "gc.auto=0",
        "-c",
        "maintenance.auto=false",
        "-c",
        "push.followTags=false",
        "push",
        "--porcelain",
        "--no-verify",
        "--no-follow-tags",
        "--recurse-submodules=no",
        &lease,
        "--",
        &binding.remote_name,
        &refspec,
    ];
    let arguments: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let output = run_git_unattended(&binding.working_repository, &arguments, Duration::from_secs(30)).map_err(|error| {
        if error.kind() == std::io::ErrorKind::TimedOut {
            PublicationError::Interrupted(format!("local review publication was interrupted; inspect the receiving branch before retrying: {error}"))
        } else { PublicationError::Failed(error.to_string()) }
    })?;
    if !output.status.success() {
        return Err(PublicationError::Failed(
            git_failure(&arguments, &output).to_string(),
        ));
    }
    Ok(())
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
    git2::Oid::from_str(oid).map_err(|error| error.to_string())?;
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    let source = source.to_str().ok_or("review source path is not UTF-8")?;
    if git(receiver, &["ls-remote", "--get-url", "--", source])?.trim() != source {
        return Err("local review source URL was rewritten to a different destination".into());
    }
    git(
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
            oid,
        ],
    )?;
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    repository
        .find_commit(git2::Oid::from_str(oid).map_err(|error| error.to_string())?)
        .map(|_| ())
        .map_err(|error| format!("review commit unavailable after import: {error}"))
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
    let mut transaction = repository
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .lock_ref(reference)
        .map_err(|error| error.to_string())?;
    let current = match repository.find_reference(reference) {
        Ok(current) => current,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if current.target().map(|oid| oid.to_string()).as_deref() != Some(expected) {
        return Err(format!("review ref changed: {reference}"));
    }
    transaction
        .remove(reference)
        .and_then(|()| transaction.commit())
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SnapshotPins {
    task_id: String,
    snapshot_id: String,
    directory_id: String,
    head: String,
    source_base: String,
    comparison_base: String,
}

fn snapshot_marker(binding: &ReviewBranchBinding, task_id: &str, snapshot_id: &str) -> PathBuf {
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

fn plan_snapshot_pins(
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
    let prefix = super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
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
    saved.uncommitted_files = Some(super::capture::count_uncommitted(&working)?);
    Ok(())
}

fn create_expected_pin(
    repository: &git2::Repository,
    name: &str,
    expected: &str,
) -> Result<(), String> {
    let oid = git2::Oid::from_str(expected).map_err(|error| error.to_string())?;
    let mut transaction = repository
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .lock_ref(name)
        .map_err(|error| error.to_string())?;
    match repository.find_reference(name) {
        Ok(reference) if reference.target() == Some(oid) => return Ok(()),
        Ok(_) => return Err(format!("review pin changed: {name}")),
        Err(error) if error.code() == git2::ErrorCode::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    transaction
        .set_target(name, oid, None, "Build review publication")
        .and_then(|()| transaction.commit())
        .map_err(|error| error.to_string())
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
        let prefix = super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
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
    let prefix = super::capture::pin_prefix(task_id, snapshot_id, &directory.id)?;
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
    let prefix = super::capture::pin_prefix(task_id, snapshot_id, &binding.directory_id)?;
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
        let (lock, _) =
            super::receivers::locks::acquire_git_lock(path, Path::new(&format!("{name}.lock")))?;
        locks.push(lock);
    }
    for binding in bindings {
        validate_initial_pins(&repository, task_id, snapshot_id, binding)?;
    }
    let result = lock_receivers(&groups[1..], task_id, snapshot_id, publish);
    drop(locks);
    result
}

#[cfg(test)]
#[path = "publication/tests.rs"]
mod tests;
