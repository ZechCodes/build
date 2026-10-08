//! Durable ownership of per-command PR hook directories.
use crate::git_process::GitProcessEvent;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

mod files;
mod process;
use files::{Identity, MARKER};
use process::Process;

const REGISTRY: &str = "build-review-fences";
const PREFIX: &str = "build-review-fence-";
const MAX_REGISTRY_ENTRIES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Record {
    kind: String,
    token: String,
    common: PathBuf,
    common_identity: Identity,
    registry_identity: Identity,
    directory_identity: Identity,
    marker_identity: Identity,
    checkout: PathBuf,
    target_ref: String,
    expected_head: String,
    owner: Process,
    launch: Launch,
    sealed: bool,
    entries: Vec<files::Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum Launch {
    Ready,
    Unverified,
    Observed(Process),
    Finished(Process),
}
impl Launch {
    fn quiet(&self) -> bool {
        match self {
            Self::Ready => true,
            Self::Observed(child) | Self::Finished(child) => child.group_dead(),
            Self::Unverified => false,
        }
    }
}

pub(super) struct OwnedFence {
    path: PathBuf,
    directory: File,
    registry: File,
    marker: File,
    record: Mutex<Record>,
}

impl OwnedFence {
    pub(super) fn create(checkout: &Path) -> Result<Self, String> {
        let repository = git2::Repository::open(checkout).map_err(|error| error.to_string())?;
        let common = repository
            .commondir()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let common_file = files::directory(&common)?;
        let registry = registry(&common_file, true)?.ok_or("missing PR fence registry")?;
        let _registry_lease = registry_lease(&registry)?;
        files::names(&registry, MAX_REGISTRY_ENTRIES)?;
        let token = uuid::Uuid::new_v4().to_string();
        let name = format!("{PREFIX}{token}");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(files::at(&registry, &name))
            .map_err(|error| error.to_string())?;
        let directory = files::directory(&files::at(&registry, &name))?;
        directory.try_lock().map_err(|error| error.to_string())?;
        let marker = files::regular(&files::at(&directory, MARKER), true)?;
        fs::hard_link(
            files::at(&directory, MARKER),
            files::at(&registry, marker_pin(&token)),
        )
        .map_err(|error| error.to_string())?;
        let head = repository.head().map_err(|error| error.to_string())?;
        let record = Record {
            kind: "build-pr-hook-fence-v1".into(),
            token,
            common: common.clone(),
            common_identity: Identity::file(&common_file)?,
            registry_identity: Identity::file(&registry)?,
            directory_identity: Identity::file(&directory)?,
            marker_identity: Identity::file(&marker)?,
            checkout: checkout.canonicalize().map_err(|error| error.to_string())?,
            target_ref: head.name().ok_or("PR target HEAD has no binding")?.into(),
            expected_head: head
                .target()
                .ok_or("PR target HEAD has no commit")?
                .to_string(),
            owner: Process::capture(std::process::id())?,
            launch: Launch::Ready,
            sealed: false,
            entries: Vec::new(),
        };
        let fence = Self {
            path: common.join(REGISTRY).join(name),
            directory,
            registry,
            marker,
            record: Mutex::new(record),
        };
        fence.persist()?;
        common_file.sync_all().map_err(|error| error.to_string())?;
        Ok(fence)
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn seal(&self) -> Result<(), String> {
        let _registry_lease = registry_lease(&self.registry)?;
        let mut record = self.record.lock().unwrap();
        if record.sealed {
            return Err("PR fence was already sealed".into());
        }
        record.entries = files::snapshot(&self.directory, &self.registry, &record.token)?;
        record.sealed = true;
        persist(&self.directory, &self.registry, &self.marker, &record)
    }
    pub(super) fn begin_launch(&self) -> Result<(), String> {
        let mut record = self.record.lock().unwrap();
        if !record.sealed || !matches!(record.launch, Launch::Ready) {
            return Err("PR hook launch has no sealed ownership".into());
        }
        verify_registered_path(&self.registry, &self.directory, &record)?;
        files::verify_entries(&self.directory, &self.registry, &record.entries)?;
        record.launch = Launch::Unverified;
        persist(&self.directory, &self.registry, &self.marker, &record)
    }
    pub(super) fn observe(&self, event: GitProcessEvent) -> Result<(), String> {
        if let GitProcessEvent::Started(pid) = event {
            let child = Process::child(pid)?;
            let mut record = self.record.lock().unwrap();
            if !matches!(record.launch, Launch::Unverified) {
                return Err("unexpected PR Git child observation".into());
            }
            record.launch = Launch::Observed(child);
            persist(&self.directory, &self.registry, &self.marker, &record)?;
        }
        Ok(())
    }
    pub(super) fn finish(&self) -> Result<(), String> {
        let mut record = self.record.lock().unwrap();
        let Launch::Observed(child) = &record.launch else {
            return Err("PR Git launch outcome remains unverified".into());
        };
        if !child.group_dead() {
            return Err("PR Git child group is still alive".into());
        }
        record.launch = Launch::Finished(child.clone());
        persist(&self.directory, &self.registry, &self.marker, &record)
    }
    fn persist(&self) -> Result<(), String> {
        persist(
            &self.directory,
            &self.registry,
            &self.marker,
            &self.record.lock().unwrap(),
        )
    }
}

impl Drop for OwnedFence {
    fn drop(&mut self) {
        let Ok(_registry_lease) = registry_lease(&self.registry) else {
            return;
        };
        let record = self.record.lock().unwrap();
        if record.sealed && record.launch.quiet() {
            let _ = cleanup(&self.registry, &self.directory, &self.marker, &record);
        }
    }
}

pub(crate) fn recover_fences(source: &Path) -> Result<(), String> {
    let repository = git2::Repository::open(source).map_err(|error| error.to_string())?;
    let common = repository
        .commondir()
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let common_file = files::directory(&common)?;
    let Some(registry) = registry(&common_file, false)? else {
        return Ok(());
    };
    let _registry_lease = registry_lease(&registry)?;
    for name in files::names(&registry, MAX_REGISTRY_ENTRIES)? {
        if !name.starts_with(PREFIX) {
            continue;
        }
        let _ = recover_one(&common, &common_file, &registry, &name);
    }
    Ok(())
}

fn registry(common: &File, create: bool) -> Result<Option<File>, String> {
    let path = files::at(common, REGISTRY);
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    match files::directory(&path) {
        Ok(directory) => Ok(Some(directory)),
        Err(_)
            if fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn registry_lease(registry: &File) -> Result<File, String> {
    let lease = File::open(files::at(registry, "")).map_err(|error| error.to_string())?;
    loop {
        match lease.lock() {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(lease)
}

fn marker_pin(token: &str) -> String {
    format!(".owner-{token}")
}

fn persist(
    directory: &File,
    registry: &File,
    marker: &File,
    record: &Record,
) -> Result<(), String> {
    verify_marker(directory, registry, marker, record)?;
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    files::write_record(marker, &bytes)?;
    directory
        .sync_all()
        .and_then(|_| registry.sync_all())
        .map_err(|error| error.to_string())
}

fn verify_marker(
    directory: &File,
    registry: &File,
    marker: &File,
    record: &Record,
) -> Result<(), String> {
    let pin = files::regular(&files::at(registry, marker_pin(&record.token)), false)?;
    let current = files::regular(&files::at(directory, MARKER), false)?;
    if Identity::file(marker)? != record.marker_identity
        || Identity::file(&pin)? != record.marker_identity
        || Identity::file(&current)? != record.marker_identity
        || Identity::file(directory)? != record.directory_identity
        || Identity::file(registry)? != record.registry_identity
    {
        return Err("PR fence ownership changed; retaining the recorded artifacts".into());
    }
    Ok(())
}

fn recover_one(
    common: &Path,
    common_file: &File,
    registry: &File,
    name: &str,
) -> Result<(), String> {
    let directory = files::directory(&files::at(registry, name))?;
    directory.try_lock().map_err(|error| error.to_string())?;
    let marker = files::regular(&files::at(&directory, MARKER), false)?;
    let record = latest_record(&marker)?;
    if record.kind != "build-pr-hook-fence-v1"
        || record.common != common
        || record.common_identity != Identity::file(common_file)?
        || format!("{PREFIX}{}", record.token) != name
        || uuid::Uuid::parse_str(&record.token).is_err()
        || !record.target_ref.starts_with("refs/heads/")
        || !git2::Reference::is_valid_name(&record.target_ref)
        || git2::Oid::from_str(&record.expected_head).is_err()
        || !record.sealed
        || !record.owner.dead()
        || !record.launch.quiet()
    {
        return Err("PR fence has no settled dead owner".into());
    }
    cleanup(registry, &directory, &marker, &record)
}

fn latest_record(marker: &File) -> Result<Record, String> {
    let bytes = files::read_record(marker)?;
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .rev()
        .find_map(|frame| {
            frame
                .ends_with(b"\n")
                .then(|| serde_json::from_slice(frame).ok())
                .flatten()
        })
        .ok_or_else(|| "PR fence has no complete ownership frame".into())
}

fn cleanup(
    registry: &File,
    directory: &File,
    marker: &File,
    record: &Record,
) -> Result<(), String> {
    verify_marker(directory, registry, marker, record)?;
    let current_marker = files::regular(&files::at(directory, MARKER), false)?;
    if latest_record(&current_marker)? != *record {
        return Err("PR fence ownership record changed".into());
    }
    verify_registered_path(registry, directory, record)?;
    let name = format!("{PREFIX}{}", record.token);
    files::verify_entries(directory, registry, &record.entries)?;
    files::remove_entries(directory, registry, &record.entries)?;
    verify_marker(directory, registry, marker, record)?;
    // The backing inodes stay linked until the authority to remove them is
    // durably gone. A crashed cleanup can never authorize a reused inode.
    files::unlink(directory, MARKER, false)?;
    directory.sync_all().map_err(|error| error.to_string())?;
    files::unlink(registry, &marker_pin(&record.token), false)?;
    registry.sync_all().map_err(|error| error.to_string())?;
    files::remove_pins(registry, &record.entries)?;
    let registered = files::directory(&files::at(registry, &name))?;
    if Identity::file(&registered)? == record.directory_identity {
        files::unlink(registry, &name, true)?;
    }
    registry.sync_all().map_err(|error| error.to_string())
}

fn verify_registered_path(
    registry: &File,
    directory: &File,
    record: &Record,
) -> Result<(), String> {
    let common = files::directory(&record.common)?;
    let registered_registry = files::directory(&record.common.join(REGISTRY))?;
    let registered_directory = files::directory(
        &record
            .common
            .join(REGISTRY)
            .join(format!("{PREFIX}{}", record.token)),
    )?;
    if Identity::file(&common)? != record.common_identity
        || Identity::file(&registered_registry)? != record.registry_identity
        || Identity::file(&registered_directory)? != record.directory_identity
        || Identity::file(registry)? != record.registry_identity
        || Identity::file(directory)? != record.directory_identity
    {
        return Err("PR fence directory binding was replaced".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
