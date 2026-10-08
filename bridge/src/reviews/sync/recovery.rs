//! One outstanding sync capture per task. Kernel locks exclude live writers;
//! durable candidate identity makes abrupt death recoverable without scanning
//! receiver markers or retained snapshot history.
use crate::reviews::model::ReviewBranchBinding;
use crate::reviews::publication;
use crate::reviews::receivers::{validate_registered_receiver, write_owned_json};
use crate::store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, Metadata};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    task_id: String,
    snapshot_id: String,
}

struct Entry {
    candidate: Candidate,
    identity: Metadata,
}

pub(crate) struct CaptureJournal {
    directory: File,
    path: PathBuf,
    task_id: String,
    active: Option<Entry>,
}

impl CaptureJournal {
    pub(crate) fn acquire(task_id: &str, bindings: &[ReviewBranchBinding]) -> Result<Self, String> {
        Self::acquire_with_wait(task_id, bindings, Duration::ZERO)
    }

    pub(crate) fn acquire_with_wait(
        task_id: &str,
        bindings: &[ReviewBranchBinding],
        wait: Duration,
    ) -> Result<Self, String> {
        let binding = bindings
            .iter()
            .min_by_key(|binding| &binding.receiving_repository)
            .ok_or("review sync requires a registered receiver")?;
        validate_registered_receiver(binding)?;
        let root = ensure_child(&binding.receiving_repository, "build-review-sync")?;
        let identity = format!("{:x}", Sha256::digest(task_id.as_bytes()));
        let path = ensure_child(&root, &identity)?;
        let directory = File::open(&path).map_err(|error| error.to_string())?;
        lock_capture(&directory, task_id, wait)?;
        let journal = Self {
            directory,
            path: path.join("candidate.json"),
            task_id: task_id.into(),
            active: None,
        };
        // A failed identity check still ends the acquired native lease.
        journal.validate_directory()?;
        Ok(journal)
    }

    pub(crate) fn recover(
        &self,
        store: &Store,
        bindings: &[ReviewBranchBinding],
    ) -> Result<bool, String> {
        self.validate_directory()?;
        let registered = store
            .load_review_sync_candidate(&self.task_id)
            .map_err(|error| error.to_string())?;
        let entry = self.read()?;
        let Some(snapshot_id) = registered else {
            if entry.is_some() {
                return Err("review sync candidate journal has no registry owner".into());
            }
            return Ok(false);
        };
        validate_uuid(&snapshot_id)?;
        if entry
            .as_ref()
            .is_some_and(|entry| entry.candidate.snapshot_id != snapshot_id)
        {
            return Err("review sync candidate journal and registry identities differ".into());
        }
        self.finish_registry(store, bindings, &snapshot_id, entry.as_ref())
    }

    pub(crate) fn begin(&mut self, store: &Store, snapshot_id: &str) -> Result<(), String> {
        self.validate_directory()?;
        store
            .register_review_sync_candidate(&self.task_id, snapshot_id)
            .map_err(|error| error.to_string())?;
        checkpoint(&self.task_id, "before-journal");
        let candidate = Candidate {
            task_id: self.task_id.clone(),
            snapshot_id: snapshot_id.into(),
        };
        if !write_owned_json(&self.path, &candidate)? {
            return Err("review sync candidate journal is already occupied".into());
        }
        let saved = self
            .read()?
            .ok_or("review sync candidate journal is missing")?;
        if saved.candidate != candidate {
            return Err("review sync candidate identity changed".into());
        }
        self.active = Some(saved);
        Ok(())
    }

    pub(crate) fn finish(
        &self,
        store: &Store,
        bindings: &[ReviewBranchBinding],
    ) -> Result<bool, String> {
        let entry = self
            .active
            .as_ref()
            .ok_or("review sync capture has no candidate")?;
        self.finish_registry(store, bindings, &entry.candidate.snapshot_id, Some(entry))
    }

    fn finish_registry(
        &self,
        store: &Store,
        bindings: &[ReviewBranchBinding],
        snapshot_id: &str,
        entry: Option<&Entry>,
    ) -> Result<bool, String> {
        self.verify_optional(entry)?;
        let registered = store
            .load_review_sync_candidate(&self.task_id)
            .map_err(|error| error.to_string())?;
        if registered.as_deref() != Some(snapshot_id) {
            return Err("review sync candidate registry changed before cleanup".into());
        }
        let published = store
            .review_snapshot_is_published(snapshot_id)
            .map_err(|error| error.to_string())?;
        if !published {
            publication::cleanup_received_pins(&self.task_id, snapshot_id, bindings)?;
        }
        if let Some(entry) = entry {
            self.clear(entry)?;
        } else {
            self.verify_optional(None)?;
        }
        store
            .clear_review_sync_candidate(&self.task_id, snapshot_id)
            .map_err(|error| error.to_string())?;
        Ok(published)
    }

    fn verify_optional(&self, entry: Option<&Entry>) -> Result<(), String> {
        if let Some(entry) = entry {
            return self.verify(entry);
        }
        self.validate_directory()?;
        if self.read()?.is_some() {
            return Err("review sync candidate journal appeared during recovery".into());
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<Entry>, String> {
        let identity = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !identity.file_type().is_file() {
            return Err("review sync candidate journal is not a regular file".into());
        }
        let candidate: Candidate =
            serde_json::from_slice(&fs::read(&self.path).map_err(|error| error.to_string())?)
                .map_err(|error| format!("review sync candidate journal is invalid: {error}"))?;
        validate_uuid(&candidate.snapshot_id)?;
        if candidate.task_id != self.task_id {
            return Err("review sync candidate identity changed".into());
        }
        let current = fs::symlink_metadata(&self.path).map_err(|error| error.to_string())?;
        if !same_identity(&identity, &current) {
            return Err("review sync candidate journal was replaced".into());
        }
        Ok(Some(Entry {
            candidate,
            identity,
        }))
    }

    fn verify(&self, expected: &Entry) -> Result<(), String> {
        self.validate_directory()?;
        let current = self
            .read()?
            .ok_or("review sync candidate journal is missing")?;
        if current.candidate != expected.candidate
            || !same_identity(&current.identity, &expected.identity)
        {
            return Err("review sync candidate journal changed before cleanup".into());
        }
        Ok(())
    }

    fn clear(&self, expected: &Entry) -> Result<(), String> {
        self.verify(expected)?;
        fs::remove_file(&self.path).map_err(|error| error.to_string())?;
        self.directory
            .sync_all()
            .map_err(|error| error.to_string())?;
        checkpoint(&self.task_id, "after-journal-clear");
        Ok(())
    }

    fn validate_directory(&self) -> Result<(), String> {
        same_directory(
            &self.directory,
            self.path
                .parent()
                .ok_or("review sync journal parent is missing")?,
        )
    }
}

impl Drop for CaptureJournal {
    fn drop(&mut self) {
        // Close alone leaves a flock active in forked children sharing this
        // file description. The capture lifetime ends here, before any close.
        loop {
            match self.directory.unlock() {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
    }
}

fn lock_capture(directory: &File, _task_id: &str, wait: Duration) -> Result<(), String> {
    let deadline = Instant::now()
        .checked_add(wait)
        .ok_or("review sync capture wait exceeds its supported duration")?;
    loop {
        match directory.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                #[cfg(test)]
                super::recovery_tests::capture_blocked(_task_id);
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err("busy: review sync capture".into());
                }
                std::thread::sleep(remaining.min(Duration::from_millis(25)));
            }
            Err(error) => return Err(format!("review sync capture lock failed: {error}")),
        }
    }
}

fn validate_uuid(snapshot_id: &str) -> Result<(), String> {
    let id = uuid::Uuid::parse_str(snapshot_id)
        .map_err(|_| "review sync candidate snapshot identity is invalid")?;
    if id.to_string() != snapshot_id {
        return Err("review sync candidate snapshot identity is invalid".into());
    }
    Ok(())
}

fn ensure_child(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if !fs::symlink_metadata(&path)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_dir()
        || path.canonicalize().map_err(|error| error.to_string())? != path
    {
        return Err("review sync journal directory changed".into());
    }
    Ok(path)
}

fn same_directory(directory: &File, path: &Path) -> Result<(), String> {
    let current = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let opened = directory.metadata().map_err(|error| error.to_string())?;
    if !current.file_type().is_dir()
        || !same_identity(&opened, &current)
        || path.canonicalize().map_err(|error| error.to_string())? != path
    {
        return Err("review sync capture lock directory changed".into());
    }
    Ok(())
}

fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

pub(super) fn checkpoint(task_id: &str, phase: &str) {
    #[cfg(test)]
    {
        if std::env::var("BUILD_REVIEW_SYNC_TEST_TASK").as_deref() != Ok(task_id) {
            return;
        }
        let selected = std::env::var("BUILD_REVIEW_SYNC_DEATH_PHASE").unwrap_or_default();
        if selected == phase {
            std::process::exit(27);
        }
        if phase == "after-db" && selected == "replace-journal" {
            let path =
                std::env::var_os("BUILD_REVIEW_SYNC_TEST_JOURNAL").expect("test journal path");
            fs::write(path, b"foreign replacement journal").expect("test journal replacement");
        }
    }
    #[cfg(not(test))]
    let _ = (task_id, phase);
}
