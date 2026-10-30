//! Validate the task-state root and its ancestors before backup work.

use super::StoreError;
use std::path::Path;

#[cfg(unix)]
pub(super) fn validate_task_state_root(dir: &Path) -> Result<Vec<rustix::fd::OwnedFd>, StoreError> {
    use rustix::fs::{open, openat, Mode, OFlags};
    use std::path::{Component, PathBuf};

    let state = dir.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let absolute = if state.is_absolute() {
        state.to_path_buf()
    } else {
        std::env::current_dir()?.join(state)
    };
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = open("/", flags, Mode::empty()).map_err(|error| rejected(Path::new("/"), error))?;
    let mut handles = vec![root];
    let mut current = PathBuf::from("/");
    inspect(handles.last().unwrap(), &current)?;
    for component in absolute.components() {
        match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => {
                current.push(name);
                let next = openat(handles.last().unwrap(), name, flags, Mode::empty())
                    .map_err(|error| rejected(&current, error))?;
                inspect(&next, &current)?;
                handles.push(next);
            }
            _ => return Err(invalid(&current, "unsupported path component")),
        }
    }
    Ok(handles)
}

#[cfg(unix)]
fn inspect(fd: &rustix::fd::OwnedFd, path: &Path) -> Result<(), StoreError> {
    let stat = rustix::fs::fstat(fd).map_err(|error| rejected(path, error))?;
    let uid = unsafe { libc::geteuid() };
    let trusted_owner = stat.st_uid == 0 || stat.st_uid == uid;
    let other_writable = stat.st_mode & 0o022 != 0;
    let sticky = stat.st_mode & 0o1000 != 0;
    if !trusted_owner || (other_writable && !sticky) {
        return Err(invalid(path, "untrusted owner or writable nonsticky directory"));
    }
    Ok(())
}

#[cfg(unix)]
fn rejected(path: &Path, error: rustix::io::Errno) -> StoreError {
    invalid(path, &error.to_string())
}

#[cfg(unix)]
fn invalid(path: &Path, reason: &str) -> StoreError {
    StoreError::Io(std::io::Error::other(format!(
        "backup state component {}: {reason}",
        path.display()
    )))
}

#[cfg(not(unix))]
pub(super) fn validate_task_state_root(_dir: &Path) -> Result<(), StoreError> {
    Err(StoreError::Io(std::io::Error::other(
        "backup state ancestor validation is unsupported on this platform",
    )))
}
