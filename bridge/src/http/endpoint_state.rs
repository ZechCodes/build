//! Persist endpoint intent separately from hosted acknowledgment. No credentials.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

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
        persist(path, &record)?;
        Ok(Some(record))
    }

    pub fn acknowledge(&self, path: &Path) -> Result<(), String> {
        let mut current = read(path)?.ok_or("HTTP endpoint state disappeared")?;
        if current.endpoint_epoch != self.endpoint_epoch {
            return Err("HTTP endpoint registration was superseded".into());
        }
        current.registered_epoch = Some(self.endpoint_epoch.clone());
        persist(path, &current)
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
