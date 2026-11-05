//! Inspect the configured user directory using normal filesystem resolution.
//! Only a missing final leaf is created; existing user ancestors are not walked.
use std::fs::{DirBuilder, File};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) enum HomeError {
    Symlink,
    Unavailable,
}

pub(super) fn open_or_create(home: &Path) -> Result<File, HomeError> {
    if !home.is_absolute() {
        return Err(HomeError::Unavailable);
    }
    match std::fs::canonicalize(home) {
        Ok(resolved) => open_existing(home, resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_leaf(home),
        Err(_) => Err(HomeError::Unavailable),
    }
}

fn create_leaf(home: &Path) -> Result<File, HomeError> {
    let parent = home.parent().ok_or(HomeError::Unavailable)?;
    let resolved = std::fs::canonicalize(parent).map_err(|_| HomeError::Unavailable)?;
    if resolved != parent {
        return Err(HomeError::Symlink);
    }
    match DirBuilder::new().mode(0o700).create(home) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(HomeError::Unavailable),
    }
    let resolved = std::fs::canonicalize(home).map_err(|_| HomeError::Unavailable)?;
    open_existing(home, resolved)
}

fn open_existing(home: &Path, resolved: PathBuf) -> Result<File, HomeError> {
    if home != resolved {
        return Err(HomeError::Symlink);
    }
    let directory = File::open(home).map_err(|_| HomeError::Unavailable)?;
    let metadata = directory.metadata().map_err(|_| HomeError::Unavailable)?;
    // SAFETY: geteuid has no pointer arguments or preconditions.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(HomeError::Unavailable);
    }
    Ok(directory)
}
