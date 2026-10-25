//! Directory capabilities keep cleanup inside the homes admitted at prepare.
use super::super::StoreError;
use std::collections::HashSet;
use std::path::Path;
use std::time::SystemTime;

#[cfg(unix)]
mod unix {
    use super::*;
    use rustix::fs::{open, openat, unlinkat, AtFlags, Dir, Mode, OFlags};
    use rustix::io::Errno;
    use std::ffi::OsString;
    use std::fs::{File, Metadata};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;
    use std::sync::Arc;

    pub struct Candidate {
        directory: Arc<File>,
        name: OsString,
        device: u64,
        inode: u64,
    }

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    /// Each component is opened relative to its already pinned parent. Neither
    /// an initial ancestor symlink nor a concurrent path rename can redirect it.
    fn open_directory(home: &Path) -> Result<Option<File>, StoreError> {
        let base = if home.is_absolute() { "/" } else { "." };
        let mut directory =
            File::from(open(base, DIRECTORY_FLAGS, Mode::empty()).map_err(std::io::Error::from)?);
        for component in home.components() {
            match component {
                Component::RootDir | Component::CurDir => (),
                Component::Normal(name) => {
                    match openat(&directory, name, DIRECTORY_FLAGS, Mode::empty()) {
                        Ok(next) => directory = File::from(next),
                        Err(error) if unavailable(error) => return Ok(None),
                        Err(error) => return Err(std::io::Error::from(error).into()),
                    }
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(directory))
    }

    fn unavailable(error: Errno) -> bool {
        matches!(
            error,
            Errno::NOENT | Errno::NOTDIR | Errno::LOOP | Errno::NXIO
        )
    }

    fn leaf_metadata(
        directory: &File,
        name: &std::ffi::OsStr,
    ) -> Result<Option<Metadata>, StoreError> {
        // NONBLOCK avoids waiting if a candidate was replaced by a FIFO.
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        match openat(directory, name, flags, Mode::empty()) {
            Ok(file) => Ok(Some(File::from(file).metadata()?)),
            Err(error) if unavailable(error) => Ok(None),
            Err(error) => Err(std::io::Error::from(error).into()),
        }
    }

    pub fn collect_candidates(
        home: &Path,
        retained: &HashSet<String>,
        older_than: SystemTime,
        candidates: &mut Vec<Candidate>,
        limit: usize,
    ) -> Result<(), StoreError> {
        let Some(directory) = open_directory(home)? else {
            return Ok(());
        };
        let directory = Arc::new(directory);
        let entries = Dir::read_from(&directory).map_err(std::io::Error::from)?;
        for entry in entries {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes());
            if matches!(name.to_str(), Some("." | ".."))
                || retained.contains(&name.to_string_lossy().into_owned())
            {
                continue;
            }
            if let Some(metadata) = leaf_metadata(&directory, name)? {
                if metadata.file_type().is_file() && metadata.modified()? < older_than {
                    candidates.push(Candidate {
                        directory: directory.clone(),
                        name: name.to_owned(),
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    });
                    if candidates.len() == limit {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    impl Candidate {
        pub fn remove_if_unchanged(&self, older_than: SystemTime) -> Result<bool, StoreError> {
            let Some(metadata) = leaf_metadata(&self.directory, &self.name)? else {
                return Ok(false);
            };
            if !metadata.file_type().is_file()
                || metadata.modified()? >= older_than
                || metadata.dev() != self.device
                || metadata.ino() != self.inode
            {
                return Ok(false);
            }
            match unlinkat(&self.directory, &self.name, AtFlags::empty()) {
                Ok(()) => Ok(true),
                Err(error) if error == Errno::NOENT => Ok(false),
                Err(error) => Err(std::io::Error::from(error).into()),
            }
        }
    }
}

#[cfg(unix)]
pub(super) use unix::{collect_candidates, Candidate};

// Deleting through ambient paths cannot safely emulate directory capabilities.
// Unsupported targets preserve files until a capability implementation exists.
#[cfg(not(unix))]
pub(super) struct Candidate;
#[cfg(not(unix))]
impl Candidate {
    pub(super) fn remove_if_unchanged(&self, _: SystemTime) -> Result<bool, StoreError> {
        Ok(false)
    }
}
#[cfg(not(unix))]
pub(super) fn collect_candidates(
    _: &Path,
    _: &HashSet<String>,
    _: SystemTime,
    _: &mut Vec<Candidate>,
    _: usize,
) -> Result<(), StoreError> {
    Ok(())
}
