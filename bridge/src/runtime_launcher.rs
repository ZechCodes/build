//! Checkout-independent `buildctl` installed beside the bridge executable.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub const SOURCE: &str = include_str!("../runtime/buildctl");

/// Publish a launcher atomically. It contains no identity or runtime authority.
pub fn install_sibling(binary: &Path) -> io::Result<()> {
    let directory = binary.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let destination = directory.join("buildctl");
    if destination.symlink_metadata().is_ok() {
        let existing = fs::read_to_string(&destination)?;
        if destination.is_symlink()
            || !existing.starts_with("#!/bin/sh\n# Agent runtime facade.")
        {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
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
        fs::File::open(directory)?.sync_all()
    })();
    let _ = fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn installs_executable_sibling_and_replaces_existing_launcher() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("build-bridge");
        install_sibling(&binary).unwrap();
        let launcher = directory.path().join("buildctl");
        assert_eq!(fs::read_to_string(&launcher).unwrap(), SOURCE);
        assert_ne!(
            fs::metadata(&launcher).unwrap().permissions().mode() & 0o111,
            0
        );
        fs::write(&launcher, "#!/bin/sh\n# Agent runtime facade. old\n").unwrap();
        install_sibling(&binary).unwrap();
        assert_eq!(fs::read_to_string(launcher).unwrap(), SOURCE);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn never_replaces_an_existing_management_command() {
        let directory = tempfile::tempdir().unwrap();
        let launcher = directory.path().join("buildctl");
        fs::write(&launcher, "management command").unwrap();
        assert_eq!(
            install_sibling(&directory.path().join("build-bridge")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(launcher).unwrap(), "management command");
    }
}
