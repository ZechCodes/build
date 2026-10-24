//! Quarantine under admission; delete only an explicitly approved private copy.
use super::*;
use crate::reviews::receivers::{locks::RecoveryWait, write_owned_json};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;

#[derive(Serialize, Deserialize)]
struct Deletion {
    repository_id: String,
    token: String,
    #[serde(default)]
    approved: bool,
}

pub(super) struct Job {
    journal: PathBuf,
    trash: PathBuf,
    canonical: PathBuf,
    _lease: leases::Lease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeletionStep {
    BeforeRename,
    Quarantined,
}

impl Deletion {
    fn paths(&self, root: &Path) -> Result<(PathBuf, PathBuf), String> {
        let token = uuid::Uuid::parse_str(&self.token).map_err(|error| error.to_string())?;
        if token.to_string() != self.token
            || self.repository_id.len() != 64
            || !self
                .repository_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("invalid receiver deletion identity".into());
        }
        Ok((
            root.join("operations")
                .join(format!("delete-{}.json", self.token)),
            root.join(format!(
                ".{}.receiver-deleting-{}",
                self.repository_id, self.token
            )),
        ))
    }

    fn save(&self, journal: &Path) -> Result<(), String> {
        let temporary = journal.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            fs::rename(&temporary, journal)?;
            fs::File::open(
                journal
                    .parent()
                    .ok_or_else(|| std::io::Error::other("invalid deletion journal"))?,
            )?
            .sync_all()
        })();
        let _ = fs::remove_file(temporary);
        result.map_err(|error: std::io::Error| error.to_string())
    }
}

/// Caller holds opening.lock and has closed native admission and drained locks.
pub(super) fn prepare(root: &Path, receiver: &Path) -> Result<Job, String> {
    prepare_with_checkpoint(root, receiver, |_, _, _| Ok(()))
}

pub(super) fn prepare_with_checkpoint(
    root: &Path,
    receiver: &Path,
    mut checkpoint: impl FnMut(DeletionStep, &Path, &Path) -> Result<(), String>,
) -> Result<Job, String> {
    crate::reviews::receivers::validate_recorded_receiver_with_recovery_wait(
        receiver,
        RecoveryWait::TryOnly,
    )?;
    let references = admission::references(receiver)?;
    let objects = object_inventory(receiver)?;
    let mut record = Deletion {
        repository_id: receiver
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("invalid receiver name")?
            .into(),
        token: uuid::Uuid::new_v4().to_string(),
        approved: false,
    };
    let (journal, trash) = record.paths(root)?;
    write_owned_json(&journal, &record)?;
    let lease = acquire_job_lease(&journal)?;
    checkpoint(DeletionStep::BeforeRename, receiver, &trash)?;
    fs::rename(receiver, &trash).map_err(|error| error.to_string())?;
    sync_parent(&trash)?;
    checkpoint(DeletionStep::Quarantined, &trash, &trash)?;
    if receiver.exists() || foreign_state(&trash, &references, &objects).unwrap_or(true) {
        return Err(format!(
            "preserved foreign or late receiver state in {} and {}",
            trash.display(),
            receiver.display()
        ));
    }
    record.approved = true;
    record.save(&journal)?;
    Ok(Job {
        journal,
        trash,
        canonical: receiver.into(),
        _lease: lease,
    })
}

fn foreign_state(
    receiver: &Path,
    references: &BTreeMap<String, String>,
    objects: &BTreeSet<git2::Oid>,
) -> Result<bool, String> {
    Ok(super::worker::has_foreign_refs(receiver)?
        || &admission::references(receiver)? != references
        || &object_inventory(receiver)? != objects)
}

fn object_inventory(receiver: &Path) -> Result<BTreeSet<git2::Oid>, String> {
    let repository = git2::Repository::open_bare(receiver).map_err(|error| error.to_string())?;
    let mut objects = BTreeSet::new();
    repository
        .odb()
        .map_err(|error| error.to_string())?
        .foreach(|oid| {
            objects.insert(*oid);
            true
        })
        .map_err(|error| error.to_string())?;
    Ok(objects)
}

fn acquire_job_lease(journal: &Path) -> Result<leases::Lease, String> {
    let path = journal.with_extension("lease");
    write_owned_json(&path, &"receiver deletion lease")?;
    leases::Lease::acquire(&path)
}

fn sync_parent(path: &Path) -> Result<(), String> {
    fs::File::open(path.parent().ok_or("invalid deletion path")?)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())
}

impl Job {
    pub(super) fn journal_path(&self) -> &Path {
        &self.journal
    }
    pub(super) fn finish(self) -> Result<u64, String> {
        if self.canonical.exists() {
            return Err(format!(
                "preserved reappeared canonical receiver {} and quarantine {}",
                self.canonical.display(),
                self.trash.display()
            ));
        }
        let bytes = size::allocated(&self.trash)?;
        match fs::remove_dir_all(&self.trash) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        // A late canonical writer is foreign. Never follow or remove that path.
        if self.canonical.exists() {
            eprintln!(
                "review receivers: preserved foreign canonical path after deletion: {}",
                self.canonical.display()
            );
        }
        fs::remove_file(&self.journal).map_err(|error| error.to_string())?;
        fs::remove_file(self.journal.with_extension("lease")).map_err(|error| error.to_string())?;
        sync_parent(&self.journal)?;
        Ok(bytes)
    }
}

pub(super) fn resume(root: &Path, after: Option<&Path>) -> Result<Option<Job>, String> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root.join("operations")).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().starts_with("delete-")
            && entry.path().extension().is_some_and(|ext| ext == "json")
            && entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
        {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    let split =
        candidates.partition_point(|path| after.is_some_and(|after| path.as_path() <= after));
    for journal in candidates[split..].iter().chain(&candidates[..split]) {
        let lease = match acquire_job_lease(journal) {
            Ok(lease) => lease,
            Err(_) => continue,
        };
        match restore(root, journal, lease) {
            Ok(job) => return Ok(Some(job)),
            Err(error) => eprintln!("review receivers: preserve deletion journal: {error}"),
        }
    }
    Ok(None)
}

fn restore(root: &Path, journal: &Path, lease: leases::Lease) -> Result<Job, String> {
    let record: Deletion =
        serde_json::from_slice(&fs::read(journal).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let (expected, trash) = record.paths(root)?;
    if expected != journal {
        return Err("receiver deletion journal identity changed".into());
    }
    if !record.approved {
        return Err(format!(
            "unapproved quarantine retained: {}",
            trash.display()
        ));
    }
    Ok(Job {
        journal: journal.into(),
        trash,
        canonical: root.join(format!("{}.git", record.repository_id)),
        _lease: lease,
    })
}
