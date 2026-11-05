//! Checkout-independent `buildctl` in a private directory beside the bridge.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const SOURCE: &str = include_str!("../runtime/buildctl");
const HEADER: &str = "#!/bin/sh\n# Agent runtime facade.";

/// Only an installed Build wrapper may be prepended to an agent's PATH.
/// Foreign files, symlinks and absent launchers provide no runtime directory.
pub fn bin_directory(binary: &Path) -> Option<PathBuf> {
    if !binary.is_absolute() {
        return None;
    }
    let directory = binary.parent()?.join(".build-runtime");
    let metadata = directory.symlink_metadata().ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let launcher = directory.join("buildctl");
    if !is_runtime_launcher(&launcher)
        || launcher.metadata().ok()?.permissions().mode() & 0o111 == 0
    {
        return None;
    }
    Some(directory)
}

fn is_runtime_launcher(path: &Path) -> bool {
    let Ok(metadata) = path.symlink_metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    else {
        return false;
    };
    let mut header = String::new();
    file.take(64).read_to_string(&mut header).is_ok() && header.starts_with(HEADER)
}

fn warn_occupied() {
    eprintln!("runtime launcher destination is occupied; skipping");
}

/// Publish a launcher atomically without writing a global `buildctl` command.
/// Foreign private destinations are left intact and do not block installation.
pub fn install_sibling(binary: &Path) -> io::Result<()> {
    let directory = binary
        .parent()
        .ok_or(io::ErrorKind::InvalidInput)?
        .join(".build-runtime");
    match directory.symlink_metadata() {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            warn_occupied();
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            DirBuilder::new().mode(0o700).create(&directory)?;
        }
        Err(error) => return Err(error),
    }
    let destination = directory.join("buildctl");
    if destination.symlink_metadata().is_ok() && !is_runtime_launcher(&destination) {
        warn_occupied();
        return Ok(());
    }
    let temporary = directory.join(format!(".buildctl-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&temporary)?;
        file.write_all(SOURCE.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)?;
        fs::File::open(&directory)?.sync_all()
    })();
    let _ = fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_executable_sibling_and_replaces_existing_launcher() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        install_sibling(&binary).unwrap();
        let launcher = directory.path().join(".build-runtime/buildctl");
        assert_eq!(
            bin_directory(&binary),
            Some(directory.path().join(".build-runtime"))
        );
        assert_eq!(fs::read_to_string(&launcher).unwrap(), SOURCE);
        assert_ne!(
            fs::metadata(&launcher).unwrap().permissions().mode() & 0o111,
            0
        );
        fs::write(&launcher, "#!/bin/sh\n# Agent runtime facade. old\n").unwrap();
        install_sibling(&binary).unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), SOURCE);
        assert_eq!(
            fs::read_dir(directory.path().join(".build-runtime"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn foreign_global_buildctl_coexists_with_private_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let global = directory.path().join("buildctl");
        fs::write(&global, "management command").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(global).unwrap(), "management command");
        assert_eq!(
            fs::read_to_string(directory.path().join(".build-runtime/buildctl")).unwrap(),
            SOURCE
        );
    }

    #[test]
    fn foreign_private_runtime_is_skipped_without_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let launcher = runtime.join("buildctl");
        fs::write(&launcher, "foreign command").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), "foreign command");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn symlink_private_runtime_is_skipped_without_following_it() {
        let directory = tempfile::tempdir().unwrap();
        let foreign = directory.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, directory.path().join(".build-runtime")).unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_dir(foreign).unwrap().count(), 0);
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn symlink_private_launcher_is_skipped_without_following_it() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::create_dir(&runtime).unwrap();
        let foreign = directory.path().join("foreign");
        fs::write(&foreign, "foreign command").unwrap();
        std::os::unix::fs::symlink(&foreign, runtime.join("buildctl")).unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert!(runtime.join("buildctl").is_symlink());
        assert_eq!(fs::read_to_string(foreign).unwrap(), "foreign command");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn foreign_file_at_private_directory_is_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join(".build-runtime");
        fs::write(&runtime, "foreign file").unwrap();
        install_sibling(&directory.path().join("build-bridge")).unwrap();
        assert_eq!(fs::read_to_string(runtime).unwrap(), "foreign file");
        assert_eq!(bin_directory(&directory.path().join("build-bridge")), None);
        assert!(!directory.path().join("buildctl").exists());
    }

    #[test]
    fn missing_relative_and_nonexecutable_wrappers_provide_no_path() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        assert_eq!(bin_directory(&binary), None);
        assert_eq!(bin_directory(Path::new("build-bridge")), None);
        install_sibling(&binary).unwrap();
        let launcher = directory.path().join(".build-runtime/buildctl");
        fs::set_permissions(launcher, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(bin_directory(&binary), None);
    }
}
