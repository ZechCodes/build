//! New entries published relative to a fenced, no-follow directory descriptor.

use crate::api::ApiError;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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

    fn check_target(&self, replace: bool) -> Result<(), ApiError> {
        let Some(metadata) = stat_at(&self.directory, &self.name)? else {
            return Ok(());
        };
        if !replace {
            return Err(ApiError::already_exists(format!(
                "{} already exists",
                self.path
            )));
        }
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(ApiError::invalid_params(
                "replacement must name a regular file, without symlinks",
            ));
        }
        Ok(())
    }

    pub(crate) fn stage(self, upload_id: &str, replace: bool) -> Result<StagedFile, ApiError> {
        self.check_target(replace)?;
        let temp_name = CString::new(format!(".build-upload-{upload_id}.part"))
            .expect("upload UUID has no NUL");
        // SAFETY: the parent and C string are live, flags create a new file without following symlinks.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                temp_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(entry_error("cannot create upload file"));
        }
        // SAFETY: openat returned a new owned descriptor.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let file = unsafe { File::from_raw_fd(fd) };
        let staged = StagedFile {
            destination: self,
            temp_name,
            file,
        };
        staged
            .file
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| ApiError::internal(format!("cannot set upload file mode: {error}")))?;
        Ok(staged)
    }
}

pub(crate) struct StagedFile {
    destination: Destination,
    temp_name: CString,
    file: File,
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        // A reused temp name belongs to the process that replaced it, not this upload.
        if !entry_matches(&self.destination.directory, &self.temp_name, &self.file) {
            return;
        }
        // SAFETY: unlinkat only removes our entry in the still-owned directory.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe {
            libc::unlinkat(
                self.destination.directory.as_raw_fd(),
                self.temp_name.as_ptr(),
                0,
            )
        };
    }
}

fn stat_at(directory: &File, name: &CString) -> Result<Option<libc::stat>, ApiError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstatat initializes metadata on success. The name/descriptor remain live and symlinks are not followed.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(ApiError::internal(format!(
            "cannot inspect destination: {error}"
        )));
    }
    // SAFETY: the successful fstatat initialized the value.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    Ok(Some(unsafe { metadata.assume_init() }))
}

fn entry_matches(directory: &File, name: &CString, file: &File) -> bool {
    let Ok(Some(entry)) = stat_at(directory, name) else {
        return false;
    };
    let Ok(opened) = file.metadata() else {
        return false;
    };
    entry.st_mode & libc::S_IFMT == libc::S_IFREG
        && entry.st_dev as u64 == opened.dev()
        && entry.st_ino as u64 == opened.ino()
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
