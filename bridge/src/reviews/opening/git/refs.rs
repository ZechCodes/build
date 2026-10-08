//! Git ref updates whose locks are registered before they become visible.

use super::{parse_oid, sync_directory, ReviewBranchBinding};
use crate::reviews::receivers::locks::{acquire_git_lock, recover_git_locks, OwnedGitFileLock};
use crate::reviews::receivers::refs::packed_without_reference;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

struct LockedReference {
    guard: OwnedGitFileLock,
    file: File,
}

pub(super) struct ReferenceLocks {
    references: BTreeMap<String, LockedReference>,
    common_directory: PathBuf,
    working_directory: PathBuf,
}

pub(super) fn recover(repo: &git2::Repository) -> Result<(), String> {
    recover_git_locks(repo.commondir())?;
    if repo.path() != repo.commondir() {
        recover_git_locks(repo.path())?;
    }
    Ok(())
}

impl ReferenceLocks {
    pub(super) fn acquire(
        repo: &git2::Repository,
        binding: &ReviewBranchBinding,
    ) -> Result<Self, String> {
        recover(repo)?;
        let common_directory = repo
            .commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let working_directory = repo
            .path()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let mut names = vec!["HEAD", "packed-refs", binding.dedicated_branch_ref.as_str()];
        if let Some(original) = &binding.original_branch_ref {
            names.push(original);
        }
        names.sort_unstable();
        names.dedup();
        let mut references = BTreeMap::new();
        for name in names {
            let directory = if name == "HEAD" {
                &working_directory
            } else {
                &common_directory
            };
            let (guard, file) = acquire_git_lock(directory, Path::new(&format!("{name}.lock")))?;
            references.insert(name.to_owned(), LockedReference { guard, file });
        }
        Ok(Self {
            references,
            common_directory,
            working_directory,
        })
    }

    fn write_reference(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let locked = self
            .references
            .get_mut(name)
            .ok_or("review reference was not locked")?;
        write_locked(&locked.guard, &mut locked.file, bytes)
    }

    pub(super) fn create_branch(
        &mut self,
        binding: &ReviewBranchBinding,
        message: &str,
    ) -> Result<(), String> {
        append_reflog(
            &self.common_directory,
            &binding.dedicated_branch_ref,
            git2::Oid::zero(),
            parse_oid(&binding.initial_head)?,
            message,
        )?;
        self.write_reference(
            &binding.dedicated_branch_ref,
            format!("{}\n", binding.initial_head).as_bytes(),
        )
    }

    pub(super) fn set_head(
        &mut self,
        binding: &ReviewBranchBinding,
        original: bool,
    ) -> Result<(), String> {
        let reference = if original {
            binding.original_branch_ref.as_deref()
        } else {
            Some(binding.dedicated_branch_ref.as_str())
        };
        let bytes = match reference {
            Some(reference) => format!("ref: {reference}\n"),
            None => format!("{}\n", binding.initial_head),
        };
        let oid = parse_oid(&binding.initial_head)?;
        let message = if original {
            "Cancel Build review opening"
        } else {
            "Build review opening"
        };
        append_reflog(&self.working_directory, "HEAD", oid, oid, message)?;
        self.write_reference("HEAD", bytes.as_bytes())
    }

    pub(super) fn prepare_removal(
        &self,
        binding: &ReviewBranchBinding,
    ) -> Result<Option<Vec<u8>>, String> {
        packed_without_reference(
            &self.common_directory.join("packed-refs"),
            &binding.dedicated_branch_ref,
            parse_oid(&binding.initial_head)?,
        )
    }

    pub(super) fn remove_branch(
        &mut self,
        binding: &ReviewBranchBinding,
        packed: Option<&[u8]>,
    ) -> Result<(), String> {
        if let Some(bytes) = packed {
            let locked = self
                .references
                .get_mut("packed-refs")
                .ok_or("review packed refs were not locked")?;
            locked
                .file
                .write_all(bytes)
                .and_then(|()| locked.file.sync_all())
                .map_err(|error| error.to_string())?;
        }
        self.references
            .get(&binding.dedicated_branch_ref)
            .ok_or("review branch ref was not locked")?
            .guard
            .verify_owned()?;
        self.references
            .get("packed-refs")
            .ok_or("review packed refs were not locked")?
            .guard
            .verify_owned()?;
        let loose = self.common_directory.join(&binding.dedicated_branch_ref);
        match fs::remove_file(&loose) {
            Ok(()) => sync_directory(loose.parent().ok_or("invalid review ref path")?)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        // Keep packed-refs locked until the loose ref is absent. Releasing it
        // earlier would let pack-refs recreate the entry from the loose ref.
        if packed.is_some() {
            let locked = self
                .references
                .get("packed-refs")
                .ok_or("review packed refs were not locked")?;
            locked
                .guard
                .publish_retaining_lock(&self.common_directory.join("packed-refs"))?;
        }
        Ok(())
    }
}

fn write_locked(guard: &OwnedGitFileLock, file: &mut File, bytes: &[u8]) -> Result<(), String> {
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    // Publish the registered inode while retaining the real lock through all
    // later updates. A replacement lock cannot overwrite the saved reference.
    guard.publish_retaining_lock(&guard.path().with_extension(""))
}

fn append_reflog(
    directory: &Path,
    reference: &str,
    old: git2::Oid,
    new: git2::Oid,
    message: &str,
) -> Result<(), String> {
    let relative = format!("logs/{reference}");
    let (guard, mut file) = acquire_git_lock(directory, Path::new(&format!("{relative}.lock")))?;
    let path = directory.join(relative);
    let mut bytes = read_optional(&path)?.unwrap_or_default();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("review reflog has an incomplete external entry".into());
    }
    let seconds = time::OffsetDateTime::now_utc().unix_timestamp();
    bytes.extend_from_slice(
        format!("{old} {new} Build <review@build.ing> {seconds} +0000\t{message}\n").as_bytes(),
    );
    // Publish the entire fsynced reflog before the branch ref. A death while
    // writing leaves the old log intact and only a registered temporary lock.
    write_locked(&guard, &mut file, &bytes)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            fs::read(path).map(Some).map_err(|error| error.to_string())
        }
        Ok(_) => Err("review Git metadata is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
