//! Exact-inode registry for Build-owned Codex profile cleanup.
//! Only this private directory is enumerated; user Codex homes are never scanned.
use rustix::fs::{self, AtFlags, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const MAX_RECORDS: usize = 4096;
const ADMISSION_WAIT: Duration = Duration::from_secs(1);
const ADMISSION_RETRY: Duration = Duration::from_millis(5);
static ADMISSION_MUTEX: Mutex<()> = Mutex::new(());
static OWNED_FILE_MUTEX: Mutex<()> = Mutex::new(());
const MAX_RECORD_BYTES: u64 = 16384;
const READ_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);

pub(super) struct Registry {
    directory: File,
}

/// Closing a descriptor does not end a flock retained by a pre-exec fork.
/// End the logical admission lease explicitly; profile liveness locks differ.
struct AdmissionLock {
    file: File,
    _mutex: MutexGuard<'static, ()>,
}
impl std::ops::Deref for AdmissionLock {
    type Target = File;
    fn deref(&self) -> &File {
        &self.file
    }
}
impl Drop for AdmissionLock {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

fn unlock(file: &File) {
    loop {
        match file.unlock() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            _ => break,
        }
    }
}

struct ScopedFileLock<'a>(&'a File);

impl<'a> ScopedFileLock<'a> {
    fn acquire(file: &'a File) -> io::Result<Self> {
        super::lock(file)?;
        Ok(Self(file))
    }
}

impl Drop for ScopedFileLock<'_> {
    fn drop(&mut self) {
        unlock(self.0);
    }
}

/// Production ownership acquisitions wait briefly; sweeper liveness probes do not.
/// This mutex is separate from admission because record publication holds admission.
pub(super) fn lock_owned_file(file: &File) -> io::Result<()> {
    let _owned = OWNED_FILE_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    flock_until(file, Instant::now() + ADMISSION_WAIT)
}

fn flock_until(file: &File, deadline: Instant) -> io::Result<()> {
    loop {
        match super::lock(file) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        retry_until(deadline)?;
    }
}
fn retry_until(deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "secret profile registry busy",
        ));
    }
    std::thread::sleep(remaining.min(ADMISSION_RETRY));
    Ok(())
}

fn wait_for_retry(deadline: Instant, retry: &mut Duration) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    std::thread::sleep(remaining.min(*retry));
    *retry = (*retry * 2).min(Duration::from_millis(100));
    true
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    home: PathBuf,
    parent_device: u64,
    parent_inode: u64,
    file_device: u64,
    file_inode: u64,
    uid: u32,
    final_name: String,
    pending_name: Option<String>,
}

pub(super) struct RegistryGuard {
    home: File,
    directory: File,
    _metadata: File,
    _profile: File,
    name: String,
    record: Record,
    removed: AtomicBool,
}

impl Registry {
    pub(super) fn open(root: &Path) -> io::Result<Self> {
        let directory = super::directory::open_or_create(root)?;
        private(&directory.metadata()?, true)?;
        Ok(Self { directory })
    }

    pub(super) fn record(
        &self,
        home: &Path,
        home_fd: &File,
        final_name: &str,
        file: &File,
        pending_name: Option<&str>,
    ) -> io::Result<RegistryGuard> {
        let record = Record::new(home, home_fd, final_name, file, pending_name)?;
        lock_owned_file(file)?;
        let _admission = self.admission_lock()?;
        self.admit(MAX_RECORDS)?;
        let name = format!("{}.json", uuid::Uuid::new_v4());
        let mut metadata: File = fs::openat(
            &self.directory,
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?
        .into();
        let guard = RegistryGuard {
            home: home_fd.try_clone()?,
            directory: self.directory.try_clone()?,
            _metadata: metadata.try_clone()?,
            _profile: file.try_clone()?,
            name,
            record,
            removed: AtomicBool::new(false),
        };
        lock_owned_file(&metadata)?;
        let bytes = serde_json::to_vec(&guard.record).map_err(|_| invalid())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid());
        }
        metadata.write_all(&bytes)?;
        metadata.flush()?;
        metadata.sync_all()?;
        self.directory.sync_all()?;
        Ok(guard)
    }

    pub(super) fn sweep(&self) -> io::Result<()> {
        self.sweep_with_observers(|_| {}, |_| {})
    }

    fn sweep_with_observers(
        &self,
        mut observe_retry: impl FnMut(bool),
        mut observe_metadata: impl FnMut(&File),
    ) -> io::Result<()> {
        let _admission = self.admission_lock()?;
        let deadline = Instant::now() + ADMISSION_WAIT;
        let mut retry = ADMISSION_RETRY;
        loop {
            let should_retry = self.sweep_records_observing(&mut observe_metadata)?;
            observe_retry(should_retry);
            if !should_retry {
                break;
            }
            if !wait_for_retry(deadline, &mut retry) {
                break;
            }
        }
        Ok(())
    }

    fn admission_lock(&self) -> io::Result<AdmissionLock> {
        // Serialize local admissions before starting the bounded cross-process
        // flock wait. Time spent behind another local operation is not contention
        // on this registry's file lock (and may belong to another registry root).
        let mutex = ADMISSION_MUTEX
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let deadline = Instant::now() + ADMISSION_WAIT;
        private(&self.directory.metadata()?, true)?;
        let file: File = fs::openat(
            &self.directory,
            ".lock",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::from_bits_truncate(0o600),
        )?
        .into();
        private(&file.metadata()?, false)?;
        flock_until(&file, deadline)?;
        Ok(AdmissionLock {
            file,
            _mutex: mutex,
        })
    }

    fn admit(&self, limit: usize) -> io::Result<()> {
        self.admit_with_full_observer(limit, || {})
    }

    fn admit_with_full_observer(
        &self,
        limit: usize,
        mut observe_full: impl FnMut(),
    ) -> io::Result<()> {
        if self.names()?.len() < limit {
            return Ok(());
        }
        let deadline = Instant::now() + ADMISSION_WAIT;
        let mut retry = ADMISSION_RETRY;
        loop {
            let _ = self.sweep_records()?;
            if self.names()?.len() < limit {
                return Ok(());
            }
            observe_full();
            if !wait_for_retry(deadline, &mut retry) {
                return Err(io::Error::other("secret profile registry full"));
            }
        }
    }

    fn names(&self) -> io::Result<Vec<String>> {
        let mut entries = fs::Dir::read_from(&self.directory)?;
        let mut names = Vec::new();
        let mut seen = 0;
        while let Some(entry) = entries.read() {
            seen += 1;
            if seen > MAX_RECORDS * 2 + 3 {
                return Err(invalid());
            }
            let entry = entry?;
            if let Ok(name) = entry.file_name().to_str() {
                if record_name(name) {
                    names.push(name.to_owned());
                }
            }
        }
        Ok(names)
    }

    /// Returns whether any lock kept a record live for a possible bounded retry.
    fn sweep_records(&self) -> io::Result<bool> {
        self.sweep_records_observing(|_| {})
    }

    fn sweep_records_observing(&self, mut observe_metadata: impl FnMut(&File)) -> io::Result<bool> {
        let mut locked_record = false;
        for name in self.names()? {
            locked_record |= self.sweep_record(&name, &mut observe_metadata)?;
        }
        self.directory.sync_all()?;
        Ok(locked_record)
    }

    fn sweep_record(
        &self,
        name: &str,
        mut observe_metadata: impl FnMut(&File),
    ) -> io::Result<bool> {
        let metadata: File = match fs::openat(&self.directory, name, READ_FLAGS, Mode::empty()) {
            Ok(fd) => fd.into(),
            Err(rustix::io::Errno::NOENT) => return Ok(false),
            Err(_) => {
                unlink(&self.directory, name)?;
                return Ok(false);
            }
        };
        let _metadata_lock = match ScopedFileLock::acquire(&metadata) {
            Ok(lock) => lock,
            Err(_) => return Ok(true),
        };
        observe_metadata(&metadata);
        let record = read_record(&metadata);
        if let Ok(record) = record {
            if !record.cleanup() {
                return Ok(true);
            }
        }
        unlink(&self.directory, name)?;
        Ok(false)
    }
}

impl Record {
    fn new(
        home: &Path,
        directory: &File,
        final_name: &str,
        file: &File,
        pending_name: Option<&str>,
    ) -> io::Result<Self> {
        validate_names(final_name, pending_name)?;
        if !home.is_absolute() {
            return Err(invalid());
        }
        let parent = directory.metadata()?;
        if !parent.is_dir() || parent.uid() != uid() {
            return Err(invalid());
        }
        let current = open_home(home)?;
        if !same(&current.metadata()?, parent.dev(), parent.ino()) {
            return Err(invalid());
        }
        let metadata = file.metadata()?;
        private(&metadata, false)?;
        let record = Self {
            home: home.to_owned(),
            parent_device: parent.dev(),
            parent_inode: parent.ino(),
            file_device: metadata.dev(),
            file_inode: metadata.ino(),
            uid: uid(),
            final_name: final_name.to_owned(),
            pending_name: pending_name.map(str::to_owned),
        };
        if !record.matches(directory, pending_name.unwrap_or(final_name))? {
            return Err(invalid());
        }
        Ok(record)
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.pending_name
            .as_deref()
            .into_iter()
            .chain(std::iter::once(self.final_name.as_str()))
    }

    fn matches(&self, home: &File, name: &str) -> io::Result<bool> {
        let stat = match fs::statat(home, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        Ok({
            same_raw(stat.st_dev, self.file_device)
                && same_raw(stat.st_ino, self.file_inode)
                && stat.st_uid == uid()
                && stat.st_uid == self.uid
                && fs::FileType::from_raw_mode(stat.st_mode) == fs::FileType::RegularFile
        })
    }

    /// Retain only a live profile. Lookup/cleanup failures retire this metadata
    /// without touching unproven paths or blocking another registered home.
    fn cleanup(&self) -> bool {
        let Ok(home) = open_home(&self.home) else {
            return true;
        };
        let Ok(metadata) = home.metadata() else {
            return true;
        };
        if self.uid != uid()
            || metadata.uid() != uid()
            || !same(&metadata, self.parent_device, self.parent_inode)
        {
            return true;
        }
        let Ok(Some(file)) = self.open_profile(&home) else {
            return true;
        };
        if super::lock(&file).is_err() {
            return false;
        }
        // Permission changes may prevent deletion even after identity checks.
        // The exact registered entry is best effort; user files remain untouched.
        let _ = self.unlink_profiles(&home);
        true
    }

    fn open_profile(&self, home: &File) -> io::Result<Option<File>> {
        for name in self.names() {
            if !self.matches(home, name)? {
                continue;
            }
            let file: File = match fs::openat(home, name, READ_FLAGS, Mode::empty()) {
                Ok(fd) => fd.into(),
                Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => continue,
                Err(error) => return Err(error.into()),
            };
            let metadata = file.metadata()?;
            if metadata.is_file()
                && metadata.uid() == uid()
                && same(&metadata, self.file_device, self.file_inode)
            {
                return Ok(Some(file));
            }
        }
        Ok(None)
    }

    fn unlink_profiles(&self, home: &File) -> io::Result<()> {
        for name in self.names() {
            // Identity is checked immediately before unlinkat. There is no
            // atomic conditional-unlink API; the same-user check/unlink window
            // is an accepted boundary of this exact-entry cleanup.
            if self.matches(home, name)? {
                unlink(home, name)?;
            }
        }
        home.sync_all()
    }
}

impl RegistryGuard {
    pub(super) fn remove(&self) -> io::Result<()> {
        if self.removed.load(Ordering::Acquire) {
            return Ok(());
        }
        let profile_result = self.record.unlink_profiles(&self.home);
        // Session end retires the private record even if user permissions now
        // prevent deleting the profile. Its capability is revoked independently.
        unlink(&self.directory, &self.name)?;
        self.directory.sync_all()?;
        self.removed.store(true, Ordering::Release);
        profile_result
    }
}
impl Drop for RegistryGuard {
    fn drop(&mut self) {
        let _ = self.remove();
        // No independent owner holds the metadata lease. End it explicitly so
        // a forked child cannot make a crashed record look live until exec.
        unlock(&self._metadata);
    }
}

fn read_record(file: &File) -> io::Result<Record> {
    let metadata = file.metadata()?;
    private(&metadata, false)?;
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    validate_names(&record.final_name, record.pending_name.as_deref())?;
    if !record.home.is_absolute() || record.uid != uid() {
        return Err(invalid());
    }
    Ok(record)
}
fn validate_names(final_name: &str, pending: Option<&str>) -> io::Result<()> {
    let id = final_name
        .strip_prefix("build-secret-")
        .and_then(|name| name.strip_suffix(".config.toml"))
        .ok_or_else(invalid)?;
    if !canonical_uuid(id) {
        return Err(invalid());
    }
    if pending.is_some_and(|pending| pending != format!(".build-secret-{id}.pending")) {
        return Err(invalid());
    }
    Ok(())
}
fn record_name(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(canonical_uuid)
}
fn canonical_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
}
fn open_home(home: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(home)
}
fn private(metadata: &Metadata, directory: bool) -> io::Result<()> {
    if metadata.uid() != uid() || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    if directory {
        if !metadata.is_dir() {
            return Err(invalid());
        }
    } else if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid());
    }
    Ok(())
}
fn uid() -> u32 {
    // SAFETY: geteuid has no pointer arguments or preconditions.
    unsafe { libc::geteuid() }
}
fn same_raw<T: TryInto<u64>>(value: T, expected: u64) -> bool {
    value.try_into().ok() == Some(expected)
}
fn same(metadata: &Metadata, device: u64, inode: u64) -> bool {
    metadata.dev() == device && metadata.ino() == inode
}
fn invalid() -> io::Error {
    io::Error::other("secret profile registry unavailable")
}
fn unlink(directory: &File, name: &str) -> io::Result<()> {
    match fs::unlinkat(directory, name, AtFlags::empty()) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "registry/tests.rs"]
mod tests;
