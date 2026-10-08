//! Registered Git file locks whose dead owner's inode can be recovered safely.

use super::write_owned_json;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GitLockOwner {
    token: String,
    git_directory: PathBuf,
    relative_path: PathBuf,
    pid: u32,
    boot: Option<String>,
    started: Option<String>,
    device: Option<u64>,
    inode: Option<u64>,
}

pub(crate) struct OwnedGitFileLock {
    path: PathBuf,
    backing: PathBuf,
    marker: PathBuf,
    owner: GitLockOwner,
}

impl OwnedGitFileLock {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn token(&self) -> &str {
        &self.owner.token
    }

    /// Publish only the same inode registered before Git lock acquisition.
    pub(crate) fn commit_to(&self, target: &Path) -> Result<(), String> {
        if target != self.path.with_extension("") {
            return Err("owned Git lock commit target changed".into());
        }
        if !same_lock_file(&self.path, &self.owner) {
            return Err("owned Git lock was replaced; preserving the replacement".into());
        }
        fs::rename(&self.path, target).map_err(|error| error.to_string())?;
        let parent = target.parent().ok_or("invalid Git file commit target")?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())
    }
}

impl Drop for OwnedGitFileLock {
    fn drop(&mut self) {
        if remove_owned_file(&self.path, &self.owner).is_ok() {
            let _ = remove_owned_file(&self.backing, &self.owner);
            remove_owned_marker(&self.marker, &self.owner);
        }
    }
}

fn remove_owned_file(path: &Path, owner: &GitLockOwner) -> Result<(), String> {
    if same_lock_file(path, owner) {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    let parent = path.parent().ok_or("invalid owned Git lock path")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn remove_owned_marker(path: &Path, owner: &GitLockOwner) {
    let expected = serde_json::to_vec(owner).ok();
    if expected.is_some() && fs::read(path).ok() == expected {
        let _ = fs::remove_file(path);
    }
}

fn lock_file_identity(path: &Path) -> Option<(u64, u64)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn same_lock_file(path: &Path, owner: &GitLockOwner) -> bool {
    lock_file_identity(path)
        .is_some_and(|(device, inode)| owner.device == Some(device) && owner.inode == Some(inode))
}

fn process_started(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

fn owner_dead(owner: &GitLockOwner) -> bool {
    if let (Some(saved_boot), Ok(boot)) = (
        &owner.boot,
        fs::read_to_string("/proc/sys/kernel/random/boot_id"),
    ) {
        if saved_boot != boot.trim() {
            return true;
        }
    }
    if let (Some(saved_start), Some(current_start)) = (&owner.started, process_started(owner.pid)) {
        return saved_start != &current_start;
    }
    #[cfg(unix)]
    {
        if owner.pid == 0 || owner.pid > i32::MAX as u32 {
            return false;
        }
        unsafe {
            libc::kill(owner.pid as i32, 0) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn valid_lock_path(relative_path: &Path) -> bool {
    let Some(relative) = relative_path.to_str() else {
        return false;
    };
    if matches!(relative, "config.lock" | "HEAD.lock" | "packed-refs.lock") {
        return true;
    }
    relative.strip_suffix(".lock").is_some_and(|reference| {
        reference.starts_with("refs/") && git2::Reference::is_valid_name(reference)
    })
}

/// Recover only registered Git lock files with the dead holder's exact inode.
pub(crate) fn recover_git_locks(directory: &Path) -> Result<(), String> {
    let directory = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(owner) = dead_owner(&directory, &entry)? else {
            continue;
        };
        remove_owned_file(&directory.join(&owner.relative_path), &owner)?;
        remove_owned_file(
            &directory.join(format!(".build-review-git-lock-{}.tmp", owner.token)),
            &owner,
        )?;
        remove_owned_marker(&entry.path(), &owner);
    }
    Ok(())
}

fn dead_owner(directory: &Path, entry: &fs::DirEntry) -> Result<Option<GitLockOwner>, String> {
    let name = entry.file_name();
    let Some(token) = name
        .to_str()
        .and_then(|name| name.strip_prefix(".build-review-git-lock-"))
        .and_then(|name| name.strip_suffix(".json"))
    else {
        return Ok(None);
    };
    if uuid::Uuid::parse_str(token).is_err()
        || !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
    {
        return Ok(None);
    }
    let content = fs::read(entry.path()).map_err(|error| error.to_string())?;
    let Ok(owner) = serde_json::from_slice::<GitLockOwner>(&content) else {
        return Ok(None);
    };
    if owner.token != token
        || owner.git_directory != directory
        || !valid_lock_path(&owner.relative_path)
        || !owner_dead(&owner)
    {
        return Ok(None);
    }
    validate_lock_parent(directory, &owner.relative_path)?;
    Ok(Some(owner))
}

fn validate_lock_parent(directory: &Path, relative_path: &Path) -> Result<(), String> {
    let mut parent = directory.to_path_buf();
    for component in relative_path
        .parent()
        .ok_or("invalid Git lock path")?
        .components()
    {
        parent.push(component);
        match fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
        if !fs::symlink_metadata(&parent)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_dir()
        {
            return Err("owned Git lock parent is not the expected directory".into());
        }
    }
    Ok(())
}

/// Persist an inode and holder identity before exclusively linking that inode
/// into Git's real lock path. A death at any point leaves a provable reservation.
pub(crate) fn acquire_git_lock(
    directory: &Path,
    relative_path: &Path,
) -> Result<(OwnedGitFileLock, File), String> {
    if !valid_lock_path(relative_path) {
        return Err("invalid owned Git lock path".into());
    }
    let directory = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    recover_git_locks(&directory)?;
    validate_lock_parent(&directory, relative_path)?;
    let token = uuid::Uuid::new_v4().to_string();
    let backing = directory.join(format!(".build-review-git-lock-{token}.tmp"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backing)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    let identity = lock_file_identity(&backing);
    let pid = std::process::id();
    let owner = GitLockOwner {
        token: token.clone(),
        git_directory: directory.clone(),
        relative_path: relative_path.into(),
        pid,
        boot: fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|boot| boot.trim().into()),
        started: process_started(pid),
        device: identity.map(|(device, _)| device),
        inode: identity.map(|(_, inode)| inode),
    };
    let guard = OwnedGitFileLock {
        path: directory.join(relative_path),
        backing,
        marker: directory.join(format!(".build-review-git-lock-{token}.json")),
        owner,
    };
    write_owned_json(&guard.marker, &guard.owner)?;
    fs::hard_link(&guard.backing, &guard.path).map_err(|error| {
        format!("local Git file is locked; Build will only recover a dead lock it can prove it owns: {error}")
    })?;
    File::open(&directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok((guard, file))
}
