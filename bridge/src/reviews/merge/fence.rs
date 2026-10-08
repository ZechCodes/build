//! Real Git ref reservations fence SQLite completion against receive-pack.
use super::*;
use crate::reviews::receivers::{self, locks::OwnedGitFileLock};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const REF_WAIT: Duration = Duration::from_secs(2);

pub(super) fn with_refs<T>(
    review: &Review,
    intent: &ReviewMergeIntent,
    admission: bool,
    locked: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut refs: BTreeMap<PathBuf, BTreeMap<String, Option<String>>> = BTreeMap::new();
    for source in &intent.request.sources {
        let binding = review
            .bindings
            .iter()
            .find(|binding| binding.directory_id == source.directory_id)
            .ok_or("PR merge binding unavailable")?;
        receivers::validate_binding_receiver(binding)?;
        insert_ref(
            &mut refs,
            binding.receiving_repository.clone(),
            binding.receiving_ref.clone(),
            Some(source.head.clone()),
        )?;
        insert_ref(
            &mut refs,
            receivers::canonical_common_git_dir(&binding.source_repository)?,
            source.base_branch_ref.clone(),
            admission.then(|| source.expected_base_head.clone()),
        )?;
    }
    let guards = lock_refs(&refs)?;
    for (path, references) in &refs {
        let repo = git2::Repository::open(path).map_err(|error| error.to_string())?;
        for (name, expected) in references {
            let actual = repo
                .find_reference(name)
                .map_err(|error| error.to_string())?
                .target()
                .ok_or("PR merge reference became symbolic")?
                .to_string();
            if expected
                .as_ref()
                .is_some_and(|expected| expected != &actual)
            {
                return Err(format!(
                    "stale: PR receiving or target ref changed: {name}; refresh and retry"
                ));
            }
        }
    }
    for guard in &guards {
        guard.verify_owned()?;
    }
    let result = locked();
    drop(guards);
    result
}

fn insert_ref(
    refs: &mut BTreeMap<PathBuf, BTreeMap<String, Option<String>>>,
    path: PathBuf,
    name: String,
    expected: Option<String>,
) -> Result<(), String> {
    let references = refs.entry(path).or_default();
    if references
        .get(&name)
        .is_some_and(|previous| previous != &expected)
    {
        return Err("shared repository/base merge preconditions disagree".into());
    }
    references.insert(name, expected);
    Ok(())
}

fn lock_refs(
    refs: &BTreeMap<PathBuf, BTreeMap<String, Option<String>>>,
) -> Result<Vec<OwnedGitFileLock>, String> {
    let deadline = Instant::now() + REF_WAIT;
    loop {
        let result = acquire(refs);
        match result {
            Ok(guards) => return Ok(guards),
            Err(error) if error.contains("locked") && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

fn acquire(
    refs: &BTreeMap<PathBuf, BTreeMap<String, Option<String>>>,
) -> Result<Vec<OwnedGitFileLock>, String> {
    let mut guards = Vec::new();
    for (path, references) in refs {
        for name in references.keys() {
            let (guard, _) =
                receivers::locks::acquire_git_lock(path, Path::new(&format!("{name}.lock")))?;
            guards.push(guard);
        }
    }
    Ok(guards)
}

pub(super) struct MergeLease {
    directory: std::fs::File,
}

impl Drop for MergeLease {
    fn drop(&mut self) {
        // Closing only our descriptor leaves flock held by a concurrent child's
        // inherited copy. Ending this guard must end ownership on every return.
        loop {
            match self.directory.unlock() {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
    }
}

/// The immutable intent supplies recovery identity; a concurrent retry never
/// replays a live worker. Explicit guard release also settles inherited FDs.
pub(super) fn lease(review: &Review) -> Result<MergeLease, String> {
    use sha2::{Digest, Sha256};
    let binding = review
        .bindings
        .iter()
        .min_by_key(|binding| &binding.receiving_repository)
        .ok_or("PR merge requires a registered receiver")?;
    receivers::validate_registered_receiver(binding)?;
    let root = binding.receiving_repository.join("build-review-merges");
    ensure_directory(&root)?;
    let path = root.join(format!("{:x}", Sha256::digest(review.task_id.as_bytes())));
    ensure_directory(&path)?;
    let directory = std::fs::File::open(&path).map_err(|error| error.to_string())?;
    match directory.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err("busy: PR merge is already executing".into())
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(format!("PR merge lease failed: {error}"))
        }
    }
    Ok(MergeLease { directory })
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.to_string()),
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("PR merge journal placement changed".into());
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
