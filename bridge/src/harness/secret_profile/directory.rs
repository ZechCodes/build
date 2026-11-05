//! Create only the private registry leaf beneath Build's trusted state root.
use rustix::fs::{self, Mode, OFlags};
use rustix::io::Errno;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(super) fn open_or_create(root: &Path) -> io::Result<File> {
    if !root.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let parent = root.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = root.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    // State-root selection belongs to the trusted launch context. Normal path
    // resolution here preserves the user's existing parent layout.
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(parent)?;
    // SAFETY: geteuid has no pointer arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    if directory.metadata()?.uid() != uid {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    match fs::mkdirat(&directory, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(Errno::EXIST) => (),
        Err(error) => return Err(error.into()),
    }
    let leaf: File = fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    let metadata = leaf.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    // Also sync an existing entry: another process may have created it just
    // before this open and not yet completed its own parent fsync.
    directory.sync_all()?;
    Ok(leaf)
}
