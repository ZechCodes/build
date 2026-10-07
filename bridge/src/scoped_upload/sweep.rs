//! One boot-time pass over the reserved upload staging namespace, without following links.

use super::{entry_matches, open_parent, stat_at};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const MAX_ENTRIES: usize = 1_000_000;
const MAX_DEPTH: usize = 256;

pub(crate) fn remove_stale_uploads(root: &Path, active: &HashSet<String>) -> usize {
    let Ok(directory) = open_parent(root, "") else {
        return 0;
    };
    let mut remaining = MAX_ENTRIES;
    sweep_directory(&directory, active, &mut remaining, 0)
}

fn sweep_directory(
    directory: &File,
    active: &HashSet<String>,
    remaining: &mut usize,
    depth: usize,
) -> usize {
    if depth >= MAX_DEPTH {
        return 0;
    }
    let Some(mut stream) = DirectoryStream::open(directory) else {
        return 0;
    };
    let mut removed = 0;
    while *remaining > 0 {
        let Some(name) = stream.next_name() else {
            break;
        };
        *remaining -= 1;
        if matches!(name.to_bytes(), b"." | b"..") || name.to_bytes().eq_ignore_ascii_case(b".git")
        {
            continue;
        }
        removed += sweep_entry(directory, &name, active, remaining, depth);
    }
    removed
}

fn sweep_entry(
    directory: &File,
    name: &CString,
    active: &HashSet<String>,
    remaining: &mut usize,
    depth: usize,
) -> usize {
    let Ok(Some(metadata)) = stat_at(directory, name) else {
        return 0;
    };
    match metadata.st_mode & libc::S_IFMT {
        libc::S_IFDIR => {
            let Some(child) = open_entry(directory, name, libc::O_DIRECTORY) else {
                return 0;
            };
            sweep_directory(&child, active, remaining, depth + 1)
        }
        libc::S_IFREG => {
            let Some(id) = staging_id(name) else {
                return 0;
            };
            if active.contains(id) {
                return 0;
            }
            remove_orphan(directory, name)
        }
        _ => 0,
    }
}

fn staging_id(name: &CStr) -> Option<&str> {
    let name = name.to_str().ok()?;
    let id = name.strip_prefix(".build-upload-")?.strip_suffix(".part")?;
    let uuid = uuid::Uuid::parse_str(id).ok()?;
    (uuid.get_version_num() == 4 && uuid.to_string() == id).then_some(id)
}

fn remove_orphan(directory: &File, name: &CString) -> usize {
    let Some(file) = open_entry(directory, name, libc::O_NONBLOCK) else {
        return 0;
    };
    let Ok(metadata) = file.metadata() else {
        return 0;
    };
    // SAFETY: geteuid only reads the process's effective user id.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let owner = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.permissions().mode() & 0o777 != 0o600
        || !entry_matches(directory, name, &file)
    {
        return 0;
    }
    // SAFETY: the plain entry name and owned parent remain live; only the verified staging entry is unlinked.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    usize::from(unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } == 0)
}

fn open_entry(directory: &File, name: &CStr, flags: libc::c_int) -> Option<File> {
    // SAFETY: directory/name remain live, names come from readdir, and no-follow flags reject link replacements.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | flags,
        )
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: openat returned a new owned descriptor.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    Some(unsafe { File::from_raw_fd(fd) })
}

struct DirectoryStream(*mut libc::DIR);

impl DirectoryStream {
    fn open(directory: &File) -> Option<Self> {
        let fd = directory.try_clone().ok()?.into_raw_fd();
        // SAFETY: fdopendir takes ownership of the freshly duplicated directory fd on success.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            // SAFETY: failed fdopendir did not take ownership; adopt and close our duplicate.
            // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
            drop(unsafe { File::from_raw_fd(fd) });
            return None;
        }
        Some(Self(stream))
    }

    fn next_name(&mut self) -> Option<CString> {
        // SAFETY: the stream is owned, and the returned name is copied before the next readdir.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let entry = unsafe { libc::readdir(self.0) };
        if entry.is_null() {
            return None;
        }
        // SAFETY: a successful readdir supplies a NUL-terminated d_name valid until the next call.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        Some(unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_owned())
    }
}

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        // SAFETY: this stream is owned and closed exactly once.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe { libc::closedir(self.0) };
    }
}
