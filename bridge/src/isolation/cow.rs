//! The copy-on-write clone backend: a checkout made by cloning the whole
//! project directory, `.git` included, with the filesystem's own reflink so it
//! costs no disk and starts warm. [`clone_tree`] is the one platform call —
//! `clonefile` on macOS, a `FICLONE` walk on Linux — and the probe (§4.3) runs
//! it on a single file, so cloning is written in exactly one place.

use std::path::Path;

#[cfg(target_os = "macos")]
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;

/// Clone the file or directory tree at `src` to `dst`, which must not exist.
/// On macOS one `clonefile` clones a whole tree atomically. On Linux the tree
/// is walked: directories and symlinks are recreated and every regular file is
/// reflinked with `FICLONE`, and any other file type is an error. Anywhere
/// else there is no clone at all. On any failure `dst` is removed before the
/// error returns, so a failed clone leaves nothing behind.
#[cfg(target_os = "macos")]
pub(crate) fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = to_c_path(src)?;
    let destination = to_c_path(dst)?;
    let cloned = unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0) };
    if cloned != 0 {
        let error = std::io::Error::last_os_error();
        discard(dst);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn to_c_path(path: &Path) -> std::io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
}

#[cfg(target_os = "linux")]
pub(crate) fn clone_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    clone_walk(src, dst).map_err(|error| {
        discard(dst);
        error
    })
}

#[cfg(target_os = "linux")]
fn clone_walk(src: &Path, dst: &Path) -> std::io::Result<()> {
    let source = std::fs::symlink_metadata(src)?;
    let file_type = source.file_type();
    if file_type.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            clone_walk(&entry.path(), &dst.join(entry.file_name()))?;
        }
        std::fs::set_permissions(dst, source.permissions())?;
        Ok(())
    } else if file_type.is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)
    } else if file_type.is_file() {
        reflink_file(src, dst, source.permissions().mode())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "cannot clone {}: not a regular file, directory or symlink",
                src.display()
            ),
        ))
    }
}

#[cfg(target_os = "linux")]
fn reflink_file(src: &Path, dst: &Path, mode: u32) -> std::io::Result<()> {
    let source = std::fs::File::open(src)?;
    let destination = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(dst)?;
    let cloned = unsafe { libc::ioctl(destination.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    if cloned != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn clone_tree(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "copy-on-write cloning is only available on macOS and Linux",
    ))
}

/// Remove whatever a failed clone left at `dst` — a partial tree or a single
/// file — so `clone_tree` never leaves a half-made destination behind.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn discard(dst: &Path) {
    if std::fs::remove_dir_all(dst).is_err() {
        let _ = std::fs::remove_file(dst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isolation::probe::cow_availability;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStrExt;

    /// Whether this volume can clone. On failure it prints the reason and
    /// returns false, so a clone test says aloud why it did nothing rather than
    /// passing without exercising anything. Every clone test opens with it.
    fn cow_or_skip(dir: &Path) -> bool {
        let project = dir.join("probe-project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let worktrees_root = dir.join("probe-worktrees");
        match cow_availability(&project, &worktrees_root) {
            Ok(()) => true,
            Err(reason) => {
                eprintln!("skipping: {reason}");
                false
            }
        }
    }

    #[test]
    fn clone_tree_reproduces_a_symlink_and_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(src.join("subdir")).unwrap();
        std::fs::write(src.join("subdir").join("file.txt"), b"warm").unwrap();
        std::os::unix::fs::symlink("subdir/file.txt", src.join("link")).unwrap();

        let dst = dir.path().join("dst");
        clone_tree(&src, &dst).unwrap();

        assert!(dst.join("subdir").is_dir());
        assert_eq!(
            std::fs::read(dst.join("subdir").join("file.txt")).unwrap(),
            b"warm"
        );
        assert_eq!(
            std::fs::read_link(dst.join("link")).unwrap(),
            Path::new("subdir/file.txt")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn clone_tree_errors_on_a_fifo_and_leaves_no_destination() {
        let dir = tempfile::tempdir().unwrap();
        if !cow_or_skip(dir.path()) {
            return;
        }
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        make_fifo(&src.join("pipe"));

        let dst = dir.path().join("dst");
        let error = clone_tree(&src, &dst).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}");
        assert!(!dst.exists(), "a failed clone left a destination behind");
    }

    #[cfg(target_os = "linux")]
    fn make_fifo(path: &Path) {
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let made = unsafe { libc::mkfifo(name.as_ptr(), 0o644) };
        assert_eq!(made, 0, "mkfifo failed");
    }
}
