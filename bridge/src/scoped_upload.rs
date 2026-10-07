//! New entries published relative to a fenced, no-follow directory descriptor.

use crate::api::ApiError;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(crate) struct Destination {
    directory: File,
    name: CString,
    path: String,
}

impl Destination {
    pub(crate) fn open(root: &Path, parent: &str, name: &str) -> Result<Self, ApiError> {
        let name = plain_component(name)?;
        let directory = open_parent(root, parent)?;
        let path = if parent.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{parent}/{}", name.to_string_lossy())
        };
        Ok(Self {
            directory,
            name,
            path,
        })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn create_directory(&self) -> Result<(), ApiError> {
        // SAFETY: the directory is owned and the plain relative name is a live C string.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let result =
            unsafe { libc::mkdirat(self.directory.as_raw_fd(), self.name.as_ptr(), 0o755) };
        if result < 0 {
            return Err(entry_error("cannot create directory"));
        }
        self.directory
            .sync_all()
            .map_err(|error| ApiError::internal(format!("cannot sync directory: {error}")))
    }
}

fn plain_component(component: &str) -> Result<CString, ApiError> {
    if component.is_empty()
        || matches!(component, "." | ".." | ".git")
        || component.contains(['/', '\\'])
    {
        return Err(ApiError::invalid_params(
            "path must contain plain names without .git",
        ));
    }
    CString::new(component).map_err(|_| ApiError::invalid_params("path cannot contain NUL"))
}

fn open_parent(root: &Path, parent: &str) -> Result<File, ApiError> {
    let components = if parent.is_empty() {
        Vec::new()
    } else {
        parent
            .split('/')
            .map(plain_component)
            .collect::<Result<Vec<_>, _>>()?
    };
    crate::fs_scope::fenced_scope_path(root, parent).map_err(ApiError::invalid_params)?;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|error| ApiError::invalid_params(format!("cannot open scope root: {error}")))?;
    for component in components {
        // SAFETY: openat uses an owned directory and a live plain C string. Its new fd is adopted immediately.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(ApiError::invalid_params(format!(
                "parent must be an existing directory without symlinks: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: openat returned a new owned descriptor.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

fn entry_error(context: &str) -> ApiError {
    let error = std::io::Error::last_os_error();
    let message = format!("{context}: {error}");
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        ApiError::already_exists(message)
    } else {
        ApiError::internal(message)
    }
}
