//! Durable, Build-owned local Git repositories for published review commits.

use super::model::ReviewBranchBinding;
use crate::git_process::{git_failure, run_git_unattended};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OWNERSHIP_FILE: &str = ".build-review-receiver.json";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReceiverOwnership {
    format: u32,
    repository_id: String,
    source_common_git_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewReceiver {
    pub repository_id: String,
    pub path: PathBuf,
    pub source_common_git_dir: PathBuf,
}

pub fn canonical_common_git_dir(source_repository: &Path) -> Result<PathBuf, String> {
    let repository =
        git2::Repository::open(source_repository).map_err(|error| error.to_string())?;
    repository
        .commondir()
        .canonicalize()
        .map_err(|error| error.to_string())
}

pub fn repository_id(source_repository: &Path) -> Result<String, String> {
    Ok(identity(&canonical_common_git_dir(source_repository)?))
}

pub fn plan_receiver(
    source_repository: &Path,
    receiver_root: &Path,
) -> Result<ReviewReceiver, String> {
    if !receiver_root.is_absolute() {
        return Err("review receiver root must be an absolute path".into());
    }
    let source_common_git_dir = canonical_common_git_dir(source_repository)?;
    let repository_id = identity(&source_common_git_dir);
    let receiver_root = canonical_future_path(receiver_root)?;
    Ok(ReviewReceiver {
        path: receiver_root.join(format!("{repository_id}.git")),
        repository_id,
        source_common_git_dir,
    })
}

pub fn ensure_receiver(receiver: &ReviewReceiver) -> Result<(), String> {
    validate_placement(receiver)?;
    let parent = receiver
        .path
        .parent()
        .ok_or("invalid review receiver path")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let reservation = parent.join(format!(
        ".{}.receiver-reservation.json",
        receiver.repository_id
    ));
    if receiver.path.exists() && !reservation.exists() {
        return validate_receiver(receiver);
    }
    write_owned_json(&reservation, &ownership(receiver))?;
    if read_ownership(&reservation)? != ownership(receiver) {
        return Err("review receiver initialization ownership changed".into());
    }
    match fs::create_dir(&receiver.path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    initialize_receiver(receiver)?;
    validate_receiver(receiver)?;
    fs::remove_file(reservation).map_err(|error| error.to_string())
}

pub fn validate_binding_receiver(binding: &ReviewBranchBinding) -> Result<(), String> {
    let parent = binding
        .receiving_repository
        .parent()
        .ok_or("invalid review receiver path")?;
    let receiver = plan_receiver(&binding.source_repository, parent)?;
    if receiver.repository_id != binding.repository_id
        || receiver.path != binding.receiving_repository
    {
        return Err("review receiver repository identity changed".into());
    }
    validate_receiver(&receiver)
}

fn identity(common_git_dir: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(common_git_dir.as_os_str().as_encoded_bytes())
    )
}

/// Historical snapshots may outlive both the source and its workspace.
pub fn validate_recorded_receiver(path: &Path) -> Result<(), String> {
    let saved = read_ownership(&path.join(OWNERSHIP_FILE))?;
    validate_receiver(&ReviewReceiver {
        repository_id: saved.repository_id,
        path: path.into(),
        source_common_git_dir: saved.source_common_git_dir,
    })
}

fn canonical_future_path(path: &Path) -> Result<PathBuf, String> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| error.to_string())?;
            let name = path.file_name().ok_or("invalid review receiver root")?;
            Ok(canonical_future_path(parent)?.join(name))
        }
        Err(error) => Err(error.to_string()),
    }
}

fn validate_placement(receiver: &ReviewReceiver) -> Result<(), String> {
    if identity(&receiver.source_common_git_dir) != receiver.repository_id
        || receiver.path.file_name() != Some(OsStr::new(&format!("{}.git", receiver.repository_id)))
    {
        return Err("review receiver repository identity changed".into());
    }
    let parent = receiver
        .path
        .parent()
        .ok_or("invalid review receiver path")?;
    if canonical_future_path(parent)? != parent {
        return Err("review receiver parent path changed through a symlink".into());
    }
    if fs::symlink_metadata(&receiver.path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("review receiver path is a symlink".into());
    }
    Ok(())
}

fn ownership(receiver: &ReviewReceiver) -> ReceiverOwnership {
    ReceiverOwnership {
        format: 1,
        repository_id: receiver.repository_id.clone(),
        source_common_git_dir: receiver.source_common_git_dir.clone(),
    }
}

fn initialize_receiver(receiver: &ReviewReceiver) -> Result<(), String> {
    let marker_path = receiver.path.join(OWNERSHIP_FILE);
    if !marker_path.exists() && has_unowned_initialization_files(&receiver.path)? {
        return Err("review receiver ownership marker is missing from a nonempty directory".into());
    }
    write_owned_json(&marker_path, &ownership(receiver))?;
    if read_ownership(&marker_path)? != ownership(receiver) {
        return Err("review receiver ownership marker changed".into());
    }
    if git2::Repository::open_bare(&receiver.path).is_ok_and(|repo| !repo.is_bare()) {
        return Err("review receiver is not a bare repository".into());
    }
    git(&receiver.path, &["init", "--bare"])?;
    for (key, value) in [
        ("core.hooksPath", "/dev/null"),
        ("gc.auto", "0"),
        ("maintenance.auto", "false"),
    ] {
        git(&receiver.path, &["config", "--local", key, value])?;
    }
    Ok(())
}

fn validate_receiver(receiver: &ReviewReceiver) -> Result<(), String> {
    validate_placement(receiver)?;
    let saved = read_ownership(&receiver.path.join(OWNERSHIP_FILE))?;
    if saved != ownership(receiver) {
        return Err("review receiver ownership marker changed".into());
    }
    let repository = git2::Repository::open_bare(&receiver.path)
        .map_err(|error| format!("review receiver is not a bare repository: {error}"))?;
    if !repository.is_bare()
        || repository.commondir().canonicalize().ok().as_ref() != Some(&receiver.path)
    {
        return Err("review receiver is not the expected bare repository".into());
    }
    for alternate in ["objects/info/alternates", "objects/info/http-alternates"] {
        if fs::symlink_metadata(receiver.path.join(alternate)).is_ok() {
            return Err("review receiver must not use object alternates".into());
        }
    }
    Ok(())
}

fn has_unowned_initialization_files(path: &Path) -> Result<bool, String> {
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let temporary_id = name
            .to_str()
            .and_then(|name| name.strip_prefix(".build-review-write-"))
            .and_then(|name| name.strip_suffix(".tmp"));
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            || temporary_id.is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_ownership(path: &Path) -> Result<ReceiverOwnership, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "review receiver ownership marker is missing")?;
    if !metadata.file_type().is_file() {
        return Err("review receiver ownership marker is not a regular file".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|_| "review receiver ownership marker is invalid".into())
}

/// Publish complete immutable ownership metadata without replacing an existing
/// file. A crash leaves either the complete marker or an ignored temporary file.
pub(crate) fn write_owned_json(path: &Path, value: &impl Serialize) -> Result<bool, String> {
    let parent = path
        .parent()
        .ok_or("invalid review ownership marker path")?;
    let temporary = parent.join(format!(".build-review-write-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(&serde_json::to_vec(value).map_err(|error| error.to_string())?)
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| error.to_string())?;
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    })();
    let _ = fs::remove_file(temporary);
    result
}

struct LocalConfigLock {
    path: PathBuf,
    temporary: PathBuf,
    committed: bool,
}

impl Drop for LocalConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// git2 0.20 has no config transaction API. Honor Git's config.lock protocol
/// while validating the fresh config and atomically replacing its local file.
pub(crate) fn with_local_config_locked(
    path: &Path,
    mutate: impl FnOnce(&git2::Config, &mut git2::Config) -> Result<(), String>,
) -> Result<(), String> {
    let repository = git2::Repository::open(path).map_err(|error| error.to_string())?;
    let parent = repository.commondir();
    let config_path = parent.join("config");
    let metadata = fs::symlink_metadata(&config_path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() {
        return Err("local review repository config is not a regular file".into());
    }
    let lock_path = parent.join("config.lock");
    let mut lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".build-review-config-{}.tmp", uuid::Uuid::new_v4()));
    let mut guard = LocalConfigLock {
        path: lock_path,
        temporary,
        committed: false,
    };
    lock.set_permissions(metadata.permissions())
        .map_err(|error| error.to_string())?;
    let original = fs::read(&config_path).map_err(|error| error.to_string())?;
    let mut staged_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&guard.temporary)
        .map_err(|error| error.to_string())?;
    staged_file
        .write_all(&original)
        .map_err(|error| error.to_string())?;
    let mut staged = git2::Config::open(&guard.temporary).map_err(|error| error.to_string())?;
    let fresh = repository.config().map_err(|error| error.to_string())?;
    mutate(&fresh, &mut staged)?;
    drop(staged);
    let changed = fs::read(&guard.temporary).map_err(|error| error.to_string())?;
    lock.write_all(&changed)
        .and_then(|()| lock.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(&guard.path, &config_path).map_err(|error| error.to_string())?;
    guard.committed = true;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

pub(crate) fn git(repository: &Path, args: &[&str]) -> Result<String, String> {
    let arguments: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let output = run_git_unattended(repository, &arguments, Duration::from_secs(30))
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(git_failure(&arguments, &output).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "receivers/tests.rs"]
mod tests;
