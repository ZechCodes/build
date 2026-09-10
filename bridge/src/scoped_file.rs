//! Revision-checked, descriptor-relative replacement of files in a worktree.
//!
//! The parent directory stays open from validation through `renameat`, so a
//! concurrent symlink swap cannot redirect a save outside the scoped root.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

pub(crate) const EDITABLE_MAX_BYTES: usize = 1_048_576;

pub(crate) fn revision_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn is_editable(bytes: &[u8], complete: bool) -> bool {
    complete
        && bytes.len() <= EDITABLE_MAX_BYTES
        && !bytes.contains(&0)
        && std::str::from_utf8(bytes).is_ok()
}

#[cfg(unix)]
pub(crate) fn replace_text(
    canonical_root: &Path,
    relative_path: &str,
    expected_revision: &str,
    replacement: &[u8],
) -> Result<(), String> {
    use std::ffi::CString;
    use std::fs::{File, OpenOptions};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    if !crate::fs_scope::is_worktree_contained_path(relative_path) {
        return Err("path escapes the worktree".to_string());
    }
    if relative_path
        .split('/')
        .any(|component| component == ".git")
    {
        return Err("refusing to edit git metadata".to_string());
    }
    if !is_editable(replacement, true) {
        return Err("replacement must be UTF-8 text no larger than 1048576 bytes".to_string());
    }

    let mut components = relative_path.split('/').peekable();
    let leaf_name = components
        .next_back()
        .ok_or_else(|| "path must name a file".to_string())?;
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(canonical_root)
        .map_err(|error| format!("cannot open scope root: {error}"))?;
    let mut parent = root;
    for component in components {
        let name = CString::new(component).map_err(|_| "invalid path".to_string())?;
        // SAFETY: `parent` is an owned directory descriptor, `name` is a live
        // NUL-terminated string, and a successful descriptor is immediately
        // adopted by `File`.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(format!(
                "cannot open {relative_path}: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: `openat` returned a new owned descriptor.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        parent = unsafe { File::from_raw_fd(descriptor) };
    }

    let leaf = CString::new(leaf_name).map_err(|_| "invalid path".to_string())?;
    let (original, metadata) = open_regular_file_at(&parent, &leaf, relative_path)?;
    let mode = metadata.permissions().mode();
    let identity = (metadata.dev(), metadata.ino());
    let original_bytes = read_editable_file(original, relative_path)?;
    if revision_hex(&original_bytes) != expected_revision {
        return Err("revision conflict: file changed since it was opened".to_string());
    }

    let temp_name = CString::new(format!(".build-write-{}.tmp", uuid::Uuid::new_v4()))
        .expect("UUID temp name has no NUL");
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let temp_descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if temp_descriptor < 0 {
        return Err(format!(
            "cannot create save file: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let mut temp = unsafe { File::from_raw_fd(temp_descriptor) };
    let write_result = (|| {
        temp.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot preserve file mode: {error}"))?;
        temp.write_all(replacement)
            .and_then(|()| temp.sync_all())
            .map_err(|error| format!("cannot save {relative_path}: {error}"))?;

        let (current, current_metadata) = open_regular_file_at(&parent, &leaf, relative_path)?;
        if (current_metadata.dev(), current_metadata.ino()) != identity
            || revision_hex(&read_editable_file(current, relative_path)?) != expected_revision
        {
            return Err("revision conflict: file changed since it was opened".to_string());
        }

        // SAFETY: both names are valid C strings and both directory
        // descriptors remain open. `renameat` is atomic within this directory.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        let renamed = unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                temp_name.as_ptr(),
                parent.as_raw_fd(),
                leaf.as_ptr(),
            )
        };
        if renamed < 0 {
            return Err(format!(
                "cannot replace {relative_path}: {}",
                std::io::Error::last_os_error()
            ));
        }
        parent
            .sync_all()
            .map_err(|error| format!("cannot sync save directory: {error}"))
    })();

    if write_result.is_err() {
        // SAFETY: the name is relative to the still-open, fenced directory.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe { libc::unlinkat(parent.as_raw_fd(), temp_name.as_ptr(), 0) };
    }
    write_result
}

#[cfg(unix)]
fn open_regular_file_at(
    parent: &std::fs::File,
    leaf: &std::ffi::CStr,
    path: &str,
) -> Result<(std::fs::File, std::fs::Metadata), String> {
    use std::os::fd::{AsRawFd, FromRawFd};

    // SAFETY: `parent` and `leaf` remain live for the call. A successful
    // descriptor is immediately adopted by `File`.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(format!(
            "cannot open {path}: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `openat` returned a new owned descriptor.
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let file = unsafe { std::fs::File::from_raw_fd(descriptor) };
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {path}: {error}"))?;
    if !metadata.is_file() {
        return Err("not a file".to_string());
    }
    Ok((file, metadata))
}

#[cfg(unix)]
fn read_editable_file(mut file: std::fs::File, path: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(EDITABLE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {path}: {error}"))?;
    if !is_editable(&bytes, bytes.len() <= EDITABLE_MAX_BYTES) {
        return Err("file is not editable UTF-8 text".to_string());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn replacement_preserves_mode_and_checks_revision() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("script.sh");
        std::fs::write(&path, "before\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
        let revision = revision_hex(b"before\n");

        replace_text(root.path(), "script.sh", &revision, b"after\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"after\n");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o751
        );

        let error = replace_text(root.path(), "script.sh", &revision, b"lost\n").unwrap_err();
        assert!(error.contains("revision conflict"), "{error}");
    }

    #[test]
    fn replacement_refuses_symlinked_leaf_and_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked-dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("linked-file"),
        )
        .unwrap();
        let revision = revision_hex(b"secret\n");

        assert!(replace_text(
            root.path(),
            "linked-dir/secret.txt",
            &revision,
            b"changed\n"
        )
        .is_err());
        assert!(replace_text(root.path(), "linked-file", &revision, b"changed\n").is_err());
        assert_eq!(
            std::fs::read(outside.path().join("secret.txt")).unwrap(),
            b"secret\n"
        );
    }

    #[test]
    fn replacement_refuses_git_metadata() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        let config = root.path().join(".git/config");
        std::fs::write(&config, "safe\n").unwrap();

        let error = replace_text(
            root.path(),
            ".git/config",
            &revision_hex(b"safe\n"),
            b"changed\n",
        )
        .unwrap_err();
        assert!(error.contains("git metadata"), "{error}");
        assert_eq!(std::fs::read(config).unwrap(), b"safe\n");
    }
}
