//! Rename under the opening lease; remove outside it. Each independent deletion
//! has an external UUID journal and lease that survive partial directory removal.
use super::*;
use crate::reviews::receivers::write_owned_json;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Serialize, Deserialize)]
struct Deletion {
    repository_id: String,
    token: String,
}

pub(super) struct Job {
    journal: PathBuf,
    trash: PathBuf,
    _lease: leases::Lease,
}

impl Deletion {
    fn paths(&self, root: &Path) -> Result<(PathBuf, PathBuf), String> {
        let token = uuid::Uuid::parse_str(&self.token).map_err(|error| error.to_string())?;
        if token.to_string() != self.token {
            return Err("invalid receiver deletion token".into());
        }
        if self.repository_id.len() != 64
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
}

/// Caller holds opening.lock through the rename. The returned job owns no
/// admission lock and may walk/delete its tombstone at background priority.
pub(super) fn prepare(root: &Path, receiver: &Path) -> Result<Job, String> {
    crate::reviews::receivers::validate_recorded_receiver(receiver)?;
    let record = Deletion {
        repository_id: receiver
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or("invalid receiver name")?
            .into(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let (journal, trash) = record.paths(root)?;
    write_owned_json(&journal, &record)?;
    let lease = leases::Lease::acquire(&journal)?;
    fs::rename(receiver, &trash).map_err(|error| error.to_string())?;
    fs::File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(Job {
        journal,
        trash,
        _lease: lease,
    })
}

impl Job {
    pub(super) fn journal_path(&self) -> &Path {
        &self.journal
    }
    pub(super) fn finish(self) -> Result<u64, String> {
        let bytes = size::allocated(&self.trash)?;
        match fs::remove_dir_all(&self.trash) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        fs::remove_file(&self.journal).map_err(|error| error.to_string())?;
        Ok(bytes)
    }
}

pub(super) fn resume(root: &Path, after: Option<&Path>) -> Result<Option<Job>, String> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root.join("operations")).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().starts_with("delete-")
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
        let lease = match leases::Lease::acquire(journal) {
            Ok(lease) => lease,
            Err(_) => continue,
        };
        match restore(root, journal, lease) {
            Ok(job) => return Ok(Some(job)),
            Err(error) => eprintln!("review receivers: preserve invalid deletion journal: {error}"),
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
    // A pre-rename journal never authorizes deletion of the canonical path.
    Ok(Job {
        journal: journal.into(),
        trash,
        _lease: lease,
    })
}
