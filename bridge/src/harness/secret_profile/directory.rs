//! Pin the Codex home without following path-component symlinks.
use rustix::fs::{self, Mode, OFlags};
use rustix::io::Errno;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

pub(super) fn open_or_create(home: &Path) -> io::Result<File> {
    if !home.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut directory = File::open("/")?;
    for component in home.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => directory = child(&directory, name)?,
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    // SAFETY: geteuid has no pointer arguments or preconditions.
    if directory.metadata()?.uid() != unsafe { libc::geteuid() } {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(directory)
}

fn child(parent: &File, name: &OsStr) -> io::Result<File> {
    match fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(descriptor) => return Ok(descriptor.into()),
        Err(Errno::NOENT) => (),
        Err(error) => return Err(error.into()),
    }
    match fs::mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(Errno::EXIST) => (),
        Err(error) => return Err(error.into()),
    }
    Ok(fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())?.into())
}
