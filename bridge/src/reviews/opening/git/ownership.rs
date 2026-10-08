//! Durable branch ownership stamps and teardown preservation for review openings.

use super::{parse_oid, short_identity};
use crate::reviews::model::ReviewBranchBinding;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) const OWNERSHIP_DIRECTORY: &str = "build-review-openings";
const KEEPS_BRANCH: &[u8] = b"keeps-branch";

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GitDirectoryIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl GitDirectoryIdentity {
    fn read(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            path,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BranchOwnership {
    format: u32,
    task_id: String,
    request_id: String,
    directory_id: String,
    source_id: String,
    repository_id: String,
    working_repository: PathBuf,
    source_repository: PathBuf,
    initial_head: String,
    original_branch_ref: Option<String>,
    dedicated_branch_ref: String,
    base_branch_ref: String,
    working_git_dir: GitDirectoryIdentity,
    source_git_dir: GitDirectoryIdentity,
    original_teardown: Option<Vec<u8>>,
    pub(super) reflog_message: String,
}

impl BranchOwnership {
    pub(super) fn new(
        task_id: &str,
        request_id: &str,
        binding: &ReviewBranchBinding,
        working: &git2::Repository,
        source: &git2::Repository,
    ) -> Result<Self, String> {
        if task_id.is_empty() || request_id.is_empty() {
            return Err("invalid review opening ownership identity".into());
        }
        Ok(Self {
            format: 1,
            task_id: task_id.into(),
            request_id: request_id.into(),
            directory_id: binding.directory_id.clone(),
            source_id: binding.source_id.clone(),
            repository_id: binding.repository_id.clone(),
            working_repository: binding.working_repository.clone(),
            source_repository: binding.source_repository.clone(),
            initial_head: binding.initial_head.clone(),
            original_branch_ref: binding.original_branch_ref.clone(),
            dedicated_branch_ref: binding.dedicated_branch_ref.clone(),
            base_branch_ref: binding.base_branch_ref.clone(),
            working_git_dir: GitDirectoryIdentity::read(working.path())?,
            source_git_dir: GitDirectoryIdentity::read(source.commondir())?,
            original_teardown: validated_original_teardown(binding, working)?,
            reflog_message: format!(
                "Build review opening {}/{}",
                short_identity(task_id),
                short_identity(request_id)
            ),
        })
    }
}

pub(super) fn ownership_path(working: &git2::Repository, binding: &ReviewBranchBinding) -> PathBuf {
    working.commondir().join(OWNERSHIP_DIRECTORY).join(format!(
        "{}.json",
        short_identity(&binding.dedicated_branch_ref)
    ))
}

pub(super) fn read_ownership(path: &Path) -> Result<Option<BranchOwnership>, String> {
    let parent = path.parent().ok_or("invalid review ownership path")?;
    if parent.exists() && parent.canonicalize().map_err(|error| error.to_string())? != parent {
        return Err("review ownership directory placement changed".into());
    }
    let Some(bytes) = read_regular_optional(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("invalid review branch ownership stamp: {error}"))
}

fn read_regular_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::read(path).map(Some).map_err(|error| error.to_string())
        }
        Ok(_) => Err("review ownership marker is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn persist_ownership(path: &Path, ownership: &BranchOwnership) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("invalid review branch ownership path")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    if parent.canonicalize().map_err(|error| error.to_string())? != parent {
        return Err("review ownership directory placement changed".into());
    }
    let bytes = serde_json::to_vec(ownership).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".preparing-{}", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    // Link only the complete, fsynced file into the final exclusive path. A
    // crash while writing leaves an ignored temporary stamp and no owned ref.
    let published = match fs::hard_link(&temporary, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            match read_ownership(path)? {
                Some(existing) if existing == *ownership => Ok(()),
                _ => Err("review branch ownership stamp already belongs to another opening".into()),
            }
        }
        Err(error) => Err(error.to_string()),
    };
    let _ = fs::remove_file(&temporary);
    published?;
    sync_directory(parent)
}

pub(super) fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    let _ = path;
    Ok(())
}

pub(super) fn validate_ownership(
    ownership: &BranchOwnership,
    task_id: &str,
    request_id: &str,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    source: &git2::Repository,
) -> Result<(), String> {
    if ownership.task_id != task_id || ownership.request_id != request_id {
        return Err("review branch belongs to another opening".into());
    }
    validate_ownership_binding(ownership, binding, working, source)
}

pub(super) fn validate_ownership_binding(
    ownership: &BranchOwnership,
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
    source: &git2::Repository,
) -> Result<(), String> {
    let stable_fields_match = ownership.directory_id == binding.directory_id
        && ownership.source_id == binding.source_id
        && ownership.repository_id == binding.repository_id
        && ownership.working_repository == binding.working_repository
        && ownership.source_repository == binding.source_repository
        && ownership.initial_head == binding.initial_head
        && ownership.original_branch_ref == binding.original_branch_ref
        && ownership.dedicated_branch_ref == binding.dedicated_branch_ref
        && ownership.base_branch_ref == binding.base_branch_ref;
    if ownership.format != 1 || !stable_fields_match {
        return Err("review branch ownership binding changed".into());
    }
    let expected_message = format!(
        "Build review opening {}/{}",
        short_identity(&ownership.task_id),
        short_identity(&ownership.request_id)
    );
    if ownership.task_id.is_empty()
        || ownership.request_id.is_empty()
        || ownership.reflog_message != expected_message
    {
        return Err("review branch ownership operation changed".into());
    }
    if ownership.working_git_dir != GitDirectoryIdentity::read(working.path())?
        || ownership.source_git_dir != GitDirectoryIdentity::read(source.commondir())?
    {
        return Err("review repository Git directory identity changed".into());
    }
    Ok(())
}

pub(super) fn validate_created_branch(
    working: &git2::Repository,
    binding: &ReviewBranchBinding,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    let expected = parse_oid(&binding.initial_head)?;
    let branch = working
        .find_reference(&binding.dedicated_branch_ref)
        .map_err(|error| error.to_string())?;
    if branch.target() != Some(expected) {
        return Err("dedicated review branch advanced externally".into());
    }
    let log = working
        .reflog(&binding.dedicated_branch_ref)
        .map_err(|error| error.to_string())?;
    let entry = log
        .get(0)
        .ok_or("review branch has no owned creation reflog")?;
    if entry.id_old() != git2::Oid::zero()
        || entry.id_new() != expected
        || entry.message() != Some(ownership.reflog_message.as_str())
    {
        return Err("review branch creation ownership cannot be proved".into());
    }
    Ok(())
}

fn teardown_path(working: &git2::Repository) -> PathBuf {
    working
        .path()
        .join(crate::isolation::BRANCH_TEARDOWN_MARKER)
}

fn read_teardown(working: &git2::Repository) -> Result<Option<Vec<u8>>, String> {
    read_regular_optional(&teardown_path(working))
}

fn validated_original_teardown(
    binding: &ReviewBranchBinding,
    working: &git2::Repository,
) -> Result<Option<Vec<u8>>, String> {
    crate::isolation::branch_teardown(&binding.working_repository)
        .map_err(|error| error.to_string())?;
    read_teardown(working)
}

pub(super) fn validate_teardown(
    working: &git2::Repository,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    let current = read_teardown(working)?;
    if current != ownership.original_teardown && current.as_deref() != Some(KEEPS_BRANCH) {
        return Err("review branch teardown ownership changed externally".into());
    }
    Ok(())
}

pub(super) fn preserve_teardown(
    working: &git2::Repository,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    validate_teardown(working, ownership)?;
    write_teardown(working, Some(KEEPS_BRANCH))
}

pub(super) fn restore_teardown(
    working: &git2::Repository,
    ownership: &BranchOwnership,
) -> Result<(), String> {
    validate_teardown(working, ownership)?;
    write_teardown(working, ownership.original_teardown.as_deref())
}

fn write_teardown(working: &git2::Repository, bytes: Option<&[u8]>) -> Result<(), String> {
    let path = teardown_path(working);
    match bytes {
        Some(bytes) => {
            let temporary = working
                .path()
                .join(format!("build-review-teardown-{}", uuid::Uuid::new_v4()));
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| error.to_string())?;
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        }
        None => match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        },
    }
    sync_directory(working.path())
}
