//! The rosters' three changes to a disk, each durable before it returns.
//!
//! A roster that a power cut can bring back is worse than none: it resumes
//! work that had finished. So every change the rosters make — replace a file,
//! remove one, rename one onto another — ends with the directory synced, and
//! the next boot reads what the last completed call left, never an older
//! state.
//!
//! The raw operations sit behind [`Disk`] so the tests can fail one, panic in
//! one, or read back the order they ran in; the daemon only ever uses
//! [`RealDisk`].

use std::io::{self, Write};
use std::path::Path;

/// The raw operations, none of them durable on its own.
pub trait Disk: Send + Sync {
    /// Create or truncate `path`, write `body`, and sync the file.
    fn create_synced(&self, path: &Path, body: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn unlink(&self, path: &Path) -> io::Result<()>;
    /// Sync a directory, which is what makes a rename or an unlink in it
    /// survive the power going.
    fn sync_dir(&self, dir: &Path) -> io::Result<()>;
    fn read(&self, path: &Path) -> io::Result<String>;
}

/// The filesystem itself.
pub struct RealDisk;

impl Disk for RealDisk {
    fn create_synced(&self, path: &Path, body: &[u8]) -> io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(body)?;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        std::fs::File::open(dir)?.sync_all()
    }

    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// Replace `path` with `body`: a temp file beside it, synced, renamed over
/// it, and the directory synced. A reader after any kind of death finds the
/// old file or the new one, never half of either.
pub fn replace(disk: &dyn Disk, path: &Path, body: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?;
    let temp = path.with_file_name(format!(".{}.tmp", name.to_string_lossy()));
    let replaced = disk
        .create_synced(&temp, body.as_bytes())
        .and_then(|()| disk.rename(&temp, path))
        .and_then(|()| sync_parent(disk, path));
    replaced.map_err(|error| {
        let _ = disk.unlink(&temp);
        format!("write {}: {error}", path.display())
    })
}

/// Remove `path`, and sync the directory so the removal is what a boot after
/// a power cut sees. Already gone is removed; the directory is synced all the
/// same, because an earlier removal may not have been.
pub fn remove(disk: &dyn Disk, path: &Path) -> Result<(), String> {
    match disk.unlink(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(format!("remove {}: {error}", path.display()));
        }
        _ => {}
    }
    sync_parent(disk, path).map_err(|error| format!("remove {}: {error}", path.display()))
}

/// Rename `from` onto `to`, durably. Both are in one directory.
pub fn rename(disk: &dyn Disk, from: &Path, to: &Path) -> Result<(), String> {
    disk.rename(from, to)
        .and_then(|()| sync_parent(disk, to))
        .map_err(|error| format!("rename {} to {}: {error}", from.display(), to.display()))
}

fn sync_parent(disk: &dyn Disk, path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => disk.sync_dir(dir),
        _ => disk.sync_dir(Path::new(".")),
    }
}

/// A [`RealDisk`] that writes down every operation it is asked for and can be
/// told to fail, or to panic, the next few of one kind.
#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct RecordingDisk {
        log: Mutex<Vec<String>>,
        failing_creates: AtomicUsize,
        failing_dir_syncs: AtomicUsize,
        panicking_creates: AtomicUsize,
    }

    impl RecordingDisk {
        /// Every operation so far, as `op name`, with file names only.
        pub fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        pub fn clear_log(&self) {
            self.log.lock().unwrap().clear();
        }

        pub fn fail_next_creates(&self, count: usize) {
            self.failing_creates.store(count, Ordering::SeqCst);
        }

        pub fn fail_next_dir_syncs(&self, count: usize) {
            self.failing_dir_syncs.store(count, Ordering::SeqCst);
        }

        pub fn panic_on_next_create(&self) {
            self.panicking_creates.store(1, Ordering::SeqCst);
        }

        fn note(&self, entry: String) {
            self.log.lock().unwrap().push(entry);
        }

        fn take_one(counter: &AtomicUsize) -> bool {
            let mut left = counter.load(Ordering::SeqCst);
            while let Some(next) = left.checked_sub(1) {
                match counter.compare_exchange(left, next, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => return true,
                    Err(now) => left = now,
                }
            }
            false
        }
    }

    fn name(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn refused() -> io::Error {
        io::Error::other("the test disk refused")
    }

    impl Disk for RecordingDisk {
        fn create_synced(&self, path: &Path, body: &[u8]) -> io::Result<()> {
            if Self::take_one(&self.panicking_creates) {
                panic!("the test disk panicked writing {}", name(path));
            }
            if Self::take_one(&self.failing_creates) {
                self.note(format!("create-failed {}", name(path)));
                return Err(refused());
            }
            self.note(format!("create {}", name(path)));
            RealDisk.create_synced(path, body)
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.note(format!("rename {} {}", name(from), name(to)));
            RealDisk.rename(from, to)
        }

        fn unlink(&self, path: &Path) -> io::Result<()> {
            self.note(format!("unlink {}", name(path)));
            RealDisk.unlink(path)
        }

        fn sync_dir(&self, dir: &Path) -> io::Result<()> {
            if Self::take_one(&self.failing_dir_syncs) {
                self.note("sync-dir-failed".to_string());
                return Err(refused());
            }
            self.note("sync-dir".to_string());
            RealDisk.sync_dir(dir)
        }

        fn read(&self, path: &Path) -> io::Result<String> {
            RealDisk.read(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::RecordingDisk;
    use super::*;

    /// The directory is synced after the rename, or the rename is not
    /// durable; and nothing is left beside the file.
    #[test]
    fn a_replacement_syncs_the_file_then_renames_then_syncs_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let disk = RecordingDisk::default();
        replace(&disk, &dir.path().join("roster.json"), "{}").unwrap();
        assert_eq!(
            disk.log(),
            [
                "create .roster.json.tmp",
                "rename .roster.json.tmp roster.json",
                "sync-dir"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("roster.json")).unwrap(),
            "{}"
        );
    }

    /// A removal is the change most easily undone by a power cut — the file
    /// it removed was a complete, readable roster — so it too ends synced.
    #[test]
    fn a_removal_syncs_the_directory_after_the_unlink_even_when_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roster.json");
        std::fs::write(&path, "{}").unwrap();
        let disk = RecordingDisk::default();
        remove(&disk, &path).unwrap();
        remove(&disk, &path).unwrap();
        assert_eq!(
            disk.log(),
            [
                "unlink roster.json",
                "sync-dir",
                "unlink roster.json",
                "sync-dir"
            ]
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_rename_syncs_the_directory_after_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), "{}").unwrap();
        let disk = RecordingDisk::default();
        rename(&disk, &dir.path().join("a"), &dir.path().join("b")).unwrap();
        assert_eq!(disk.log(), ["rename a b", "sync-dir"]);
    }

    /// A sync that fails is reported, not swallowed: the caller must not
    /// believe a change is durable when it may not be.
    #[test]
    fn a_failed_directory_sync_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let disk = RecordingDisk::default();
        disk.fail_next_dir_syncs(1);
        assert!(remove(&disk, &dir.path().join("roster.json")).is_err());
        disk.fail_next_dir_syncs(1);
        assert!(replace(&disk, &dir.path().join("roster.json"), "{}").is_err());
    }

    /// A write that fails leaves no temp file behind.
    #[test]
    fn a_failed_replacement_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let disk = RecordingDisk::default();
        disk.fail_next_creates(1);
        assert!(replace(&disk, &dir.path().join("roster.json"), "{}").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
