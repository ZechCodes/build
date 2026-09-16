//! Materialization of ordinary, non-Git directories with the same isolation
//! preference used for checkouts.

use std::path::Path;

use super::{Isolation, IsolationBackend, ResolvedIsolation, RiftBackend, WorktreeError};

/// Copy the ordinary directory tree at `source` to the new `destination`.
///
/// `Worktree` means an independent recursive copy. `Rift` asks the configured
/// Rift CLI for a copy-on-write snapshot, and falls back to the independent
/// copy when Rift is unavailable. The destination must not exist, and any
/// partial destination made by this call is removed on error.
pub fn copy_directory(
    source: &Path,
    destination: &Path,
    requested: Isolation,
) -> Result<ResolvedIsolation, WorktreeError> {
    refuse_existing_destination(destination)?;
    let source_metadata = std::fs::metadata(source)?;
    if !source_metadata.is_dir() {
        return Err(WorktreeError::Refused(format!(
            "{} is not a directory",
            source.display()
        )));
    }
    refuse_overlapping_destination(source, destination)?;

    let root = destination.parent().ok_or_else(|| {
        WorktreeError::Refused(format!(
            "destination has no parent directory: {}",
            destination.display()
        ))
    })?;
    copy_directory_with_backend(source, destination, requested, &RiftBackend::new(root))
}

/// Copy a directory while keeping Rift's registry stable across destination
/// roots that belong to the same project.
pub fn copy_directory_with_rift_root(
    source: &Path,
    destination: &Path,
    requested: Isolation,
    rift_root: &Path,
) -> Result<ResolvedIsolation, WorktreeError> {
    refuse_existing_destination(destination)?;
    let source_metadata = std::fs::metadata(source)?;
    if !source_metadata.is_dir() {
        return Err(WorktreeError::Refused(format!(
            "{} is not a directory",
            source.display()
        )));
    }
    refuse_overlapping_destination(source, destination)?;
    let destination_root = destination.parent().ok_or_else(|| {
        WorktreeError::Refused(format!(
            "destination has no parent directory: {}",
            destination.display()
        ))
    })?;
    let backend = RiftBackend::with_registry_root(destination_root, rift_root, "rift");
    copy_directory_with_backend(source, destination, requested, &backend)
}

/// Remove an ordinary directory copy through the backend that created it.
pub(crate) fn remove_directory_with_rift_root(
    source: &Path,
    destination: &Path,
    isolation: Isolation,
    rift_root: &Path,
) -> Result<(), WorktreeError> {
    if !destination.exists() {
        return Ok(());
    }
    match isolation {
        Isolation::Worktree => std::fs::remove_dir_all(destination).map_err(Into::into),
        Isolation::Rift => {
            let destination_root = destination.parent().ok_or_else(|| {
                WorktreeError::Refused(format!(
                    "destination has no parent directory: {}",
                    destination.display()
                ))
            })?;
            RiftBackend::with_registry_root(destination_root, rift_root, "rift")
                .remove(source, destination)
        }
    }
}

fn copy_directory_with_backend(
    source: &Path,
    destination: &Path,
    requested: Isolation,
    rift: &RiftBackend,
) -> Result<ResolvedIsolation, WorktreeError> {
    match requested {
        Isolation::Worktree => {
            copy_tree(source, destination)?;
            Ok(ResolvedIsolation::honoured(Isolation::Worktree))
        }
        Isolation::Rift => match rift.directory_availability(source) {
            Ok(()) => match rift.materialize_directory(source, destination) {
                Ok(()) => Ok(ResolvedIsolation::honoured(Isolation::Rift)),
                Err(error) if !destination.exists() => {
                    copy_tree(source, destination)?;
                    Ok(ResolvedIsolation::downgraded(&error.to_string()))
                }
                Err(error) => Err(error),
            },
            Err(reason) => {
                copy_tree(source, destination)?;
                Ok(ResolvedIsolation::downgraded(&reason))
            }
        },
    }
}

fn refuse_overlapping_destination(source: &Path, destination: &Path) -> Result<(), WorktreeError> {
    let source = std::fs::canonicalize(source)?;
    let parent = destination.parent().ok_or_else(|| {
        WorktreeError::Refused(format!(
            "destination has no parent directory: {}",
            destination.display()
        ))
    })?;
    let destination_name = destination.file_name().ok_or_else(|| {
        WorktreeError::Refused(format!(
            "destination has no directory name: {}",
            destination.display()
        ))
    })?;
    let destination = std::fs::canonicalize(parent)?.join(destination_name);
    if destination.starts_with(&source) {
        return Err(WorktreeError::Refused(format!(
            "cannot copy {} inside itself at {}",
            source.display(),
            destination.display()
        )));
    }
    Ok(())
}

fn refuse_existing_destination(destination: &Path) -> Result<(), WorktreeError> {
    match std::fs::symlink_metadata(destination) {
        Ok(_) => Err(WorktreeError::Refused(format!(
            "{} already exists",
            destination.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), WorktreeError> {
    let source_metadata = std::fs::metadata(source)?;
    // Once this succeeds the root is ours. Before it succeeds, including an
    // AlreadyExists race, cleanup must not touch `destination`.
    std::fs::create_dir(destination)?;
    copy_directory_contents(source, destination)
        .and_then(|()| std::fs::set_permissions(destination, source_metadata.permissions()))
        .inspect_err(|_| discard(destination))?;
    Ok(())
}

fn copy_directory_contents(source: &Path, destination: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        copy_into(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_into(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        std::fs::create_dir(destination)?;
        copy_directory_contents(source, destination)?;
        std::fs::set_permissions(destination, metadata.permissions())
    } else if file_type.is_symlink() {
        make_symlink(&std::fs::read_link(source)?, destination)
    } else if file_type.is_file() {
        std::fs::copy(source, destination)?;
        std::fs::set_permissions(destination, metadata.permissions())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "cannot copy {}: not a regular file, directory or symlink",
                source.display()
            ),
        ))
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
fn make_symlink(target: &Path, destination: &Path) -> std::io::Result<()> {
    // Preserve the link itself. A dangling link cannot reveal which Windows
    // link kind created it, so try the file form first and report that error.
    std::os::windows::fs::symlink_file(target, destination)
}

fn discard(destination: &Path) {
    if std::fs::remove_dir_all(destination).is_err() {
        let _ = std::fs::remove_file(destination);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn ordinary_copy_preserves_contents_modes_and_symlinks_without_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let file = source.join("script");
        std::fs::write(&file, b"echo copied\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o751)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("script", source.join("script-link")).unwrap();

        let destination = dir.path().join("destination");
        let resolved = copy_directory(&source, &destination, Isolation::Worktree).unwrap();

        assert_eq!(resolved, ResolvedIsolation::honoured(Isolation::Worktree));
        assert_eq!(
            std::fs::read(destination.join("script")).unwrap(),
            b"echo copied\n"
        );
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::metadata(destination.join("script"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
            assert_eq!(
                std::fs::read_link(destination.join("script-link")).unwrap(),
                Path::new("script")
            );
            assert_ne!(
                std::fs::metadata(&file).unwrap().ino(),
                std::fs::metadata(destination.join("script")).unwrap().ino(),
                "an ordinary copy must not hard-link source files"
            );
        }
    }

    #[test]
    fn an_existing_destination_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("owned"), b"theirs").unwrap();

        let error = copy_directory(&source, &destination, Isolation::Worktree).unwrap_err();

        assert!(error.to_string().contains("already exists"), "{error}");
        assert_eq!(std::fs::read(destination.join("owned")).unwrap(), b"theirs");
    }

    #[test]
    fn losing_the_destination_creation_race_does_not_clean_up_the_winner() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        // This models another creator winning after the public preflight and
        // immediately before copy_tree atomically reserves the root.
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("winner"), b"theirs").unwrap();

        let error = copy_tree(&source, &destination).unwrap_err();

        assert!(
            error.to_string().contains("exists"),
            "unexpected error: {error}"
        );
        assert_eq!(
            std::fs::read(destination.join("winner")).unwrap(),
            b"theirs"
        );
    }

    #[test]
    fn a_missing_destination_parent_is_an_error_and_does_not_change_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("owned"), b"source").unwrap();
        let destination = dir.path().join("missing-parent").join("destination");

        let error = copy_directory(&source, &destination, Isolation::Rift).unwrap_err();

        assert!(error.to_string().contains("No such file"), "{error}");
        assert!(!destination.exists());
        assert_eq!(std::fs::read(source.join("owned")).unwrap(), b"source");
    }

    #[test]
    fn a_destination_inside_the_source_is_refused_without_creating_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("owned"), b"source").unwrap();
        let destination = source.join("nested");

        let error = copy_directory(&source, &destination, Isolation::Rift).unwrap_err();

        assert!(error.to_string().contains("inside itself"), "{error}");
        assert!(!destination.exists());
        assert_eq!(std::fs::read(source.join("owned")).unwrap(), b"source");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_inside_the_source_is_also_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::os::unix::fs::symlink(&source, dir.path().join("source-alias")).unwrap();
        let destination = dir.path().join("source-alias").join("nested");

        let error = copy_directory(&source, &destination, Isolation::Worktree).unwrap_err();

        assert!(error.to_string().contains("inside itself"), "{error}");
        assert!(!source.join("nested").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_error_removes_only_its_partial_destination() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("before"), b"still source").unwrap();
        let fifo = source.join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);

        let error = copy_directory(&source, &destination, Isolation::Worktree).unwrap_err();

        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read(source.join("before")).unwrap(),
            b"still source"
        );
    }

    #[test]
    fn rift_copies_a_plain_directory_when_available() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("file"), b"plain tree").unwrap();
        let destination = dir.path().join("destination");

        let resolved = copy_directory(&source, &destination, Isolation::Rift).unwrap();

        assert_eq!(
            std::fs::read(destination.join("file")).unwrap(),
            b"plain tree"
        );
        if resolved.isolation == Isolation::Rift {
            assert_eq!(resolved, ResolvedIsolation::honoured(Isolation::Rift));
        } else {
            assert_eq!(resolved.isolation, Isolation::Worktree);
            assert!(resolved.downgrade.is_some());
            assert!(
                !source.join(".rift").exists(),
                "a failed Rift attempt left its source marker behind"
            );
        }
    }
}
