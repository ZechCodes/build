//! Persist endpoint intent separately from hosted acknowledgment. No credentials.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointPolicy {
    pub device_id: String,
    pub issuer: String,
    pub origin: String,
    pub allowed_origins: Vec<String>,
    pub certificate_fingerprint: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointRecord {
    pub policy: EndpointPolicy,
    pub endpoint_epoch: String,
    pub enabled: bool,
    pub registered_epoch: Option<String>,
}

impl EndpointRecord {
    pub fn prepare(path: &Path, desired: Option<EndpointPolicy>) -> Result<Option<Self>, String> {
        Self::prepare_before_persist(path, desired, || {})
    }

    fn prepare_before_persist(
        path: &Path,
        desired: Option<EndpointPolicy>,
        before_persist: impl FnOnce(),
    ) -> Result<Option<Self>, String> {
        let transaction = StateTransaction::begin(path)?;
        let path = transaction.path.as_path();
        let old = read(path)?;
        let enabled = desired.is_some();
        let Some(policy) = desired.or_else(|| old.as_ref().map(|value| value.policy.clone()))
        else {
            return Ok(None);
        };
        if let Some(old) = &old {
            if old.policy == policy && old.enabled == enabled {
                return Ok(Some(old.clone()));
            }
        }
        let registered_epoch = old
            .filter(|value| {
                value.policy.device_id == policy.device_id && value.policy.issuer == policy.issuer
            })
            .and_then(|value| value.registered_epoch);
        let record = Self {
            policy,
            endpoint_epoch: uuid::Uuid::new_v4().to_string(),
            enabled,
            registered_epoch,
        };
        before_persist();
        persist(path, &record)?;
        Ok(Some(record))
    }

    pub fn acknowledge(&self, path: &Path) -> Result<(), String> {
        self.acknowledge_before_persist(path, || {})
    }

    fn acknowledge_before_persist(
        &self,
        path: &Path,
        before_persist: impl FnOnce(),
    ) -> Result<(), String> {
        let transaction = StateTransaction::begin(path)?;
        let path = transaction.path.as_path();
        let mut current = read(path)?.ok_or("HTTP endpoint state disappeared")?;
        if current.endpoint_epoch != self.endpoint_epoch {
            return Err("HTTP endpoint registration was superseded".into());
        }
        current.registered_epoch = Some(self.endpoint_epoch.clone());
        before_persist();
        persist(path, &current)
    }

    #[cfg(test)]
    pub fn test_prepare_before_persist(
        path: &Path,
        desired: Option<EndpointPolicy>,
        hook: impl FnOnce(),
    ) -> Result<Option<Self>, String> {
        Self::prepare_before_persist(path, desired, hook)
    }

    #[cfg(test)]
    pub fn test_acknowledge_before_persist(
        &self,
        path: &Path,
        hook: impl FnOnce(),
    ) -> Result<(), String> {
        self.acknowledge_before_persist(path, hook)
    }

    #[cfg(test)]
    pub fn test_released_lock_clone(path: &Path) -> Result<File, String> {
        let transaction = StateTransaction::begin(path)?;
        transaction
            ._lock
            .try_clone()
            .map_err(|_| "cannot duplicate test lock".into())
    }
}

/// One nonblocking OS transaction across independent processes and parent aliases.
/// The sibling lock file is permanent: replacing the record never replaces the
/// lock inode. Callers must run filesystem work off the async executor and bound
/// any retry of Busy; this layer never waits or retries a contended lock.
struct StateTransaction {
    path: PathBuf,
    _lock: File,
}

impl StateTransaction {
    #[cfg(unix)]
    fn begin(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let name = path
            .file_name()
            .ok_or("HTTP endpoint state requires filename")?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|_| "cannot create HTTP endpoint state directory")?;
        let parent = parent
            .canonicalize()
            .map_err(|_| "cannot resolve HTTP endpoint state directory")?;
        let path = parent.join(name);
        let mut lock_name = name.to_os_string();
        lock_name.push(".lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.join(lock_name))
            .map_err(|_| "cannot open HTTP endpoint state lock")?;
        let metadata = lock
            .metadata()
            .map_err(|_| "cannot inspect HTTP endpoint state lock")?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err("HTTP endpoint state lock requires an unaliased regular file".into());
        }
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |error| {
                if error == rustix::io::Errno::WOULDBLOCK {
                    "HTTP endpoint state is busy"
                } else {
                    "cannot lock HTTP endpoint state"
                }
            },
        )?;
        let transaction = Self { path, _lock: lock };
        validate_record_path(&transaction.path)?;
        Ok(transaction)
    }

    #[cfg(not(unix))]
    fn begin(_path: &Path) -> Result<Self, String> {
        Err("HTTP endpoint state requires OS file locking support".into())
    }
}

#[cfg(unix)]
impl Drop for StateTransaction {
    fn drop(&mut self) {
        // Explicit unlock also ends ownership if a concurrent fork temporarily
        // inherited the open file description before its close-on-exec step.
        let _ = rustix::fs::flock(&self._lock, rustix::fs::FlockOperation::Unlock);
    }
}

#[cfg(unix)]
fn validate_record_path(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => Ok(()),
        Ok(_) => Err("HTTP endpoint state requires an unaliased regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("cannot inspect HTTP endpoint state".into()),
    }
}

fn read(path: &Path) -> Result<Option<EndpointRecord>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot open HTTP endpoint state".into()),
    };
    let mut buffer = Vec::new();
    file.take(65_537)
        .read_to_end(&mut buffer)
        .map_err(|_| "cannot read HTTP endpoint state")?;
    if buffer.len() > 65_536 {
        return Err("HTTP endpoint state exceeds 64KiB".into());
    }
    let record = serde_json::from_slice(&buffer).map_err(|_| "invalid HTTP endpoint state")?;
    Ok(Some(record))
}

fn persist(path: &Path, record: &EndpointRecord) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("HTTP endpoint state requires parent directory")?;
    fs::create_dir_all(parent).map_err(|_| "cannot create HTTP endpoint state directory")?;
    let temporary = parent.join(format!(".http-endpoint-{}.tmp", uuid::Uuid::new_v4()));
    let result = persist_to(path, &temporary, record);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn persist_to(path: &Path, temporary: &Path, record: &EndpointRecord) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(temporary)
        .map_err(|_| "cannot create HTTP endpoint state")?;
    let bytes = serde_json::to_vec(record).map_err(|_| "cannot encode HTTP endpoint state")?;
    file.write_all(&bytes)
        .map_err(|_| "cannot write HTTP endpoint state")?;
    file.sync_all()
        .map_err(|_| "cannot sync HTTP endpoint state")?;
    fs::rename(temporary, path).map_err(|_| "cannot replace HTTP endpoint state")?;
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "cannot sync HTTP endpoint state directory")?;
    }
    Ok(())
}
