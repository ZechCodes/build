//! What the inventory keeps across a bridge restart: its revision and the
//! last observations, all of them nonsecret (enums, versions, provider ids).
//! Everything restored is stale until it is observed again.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::{AuthFacts, InstallationState};
use crate::models::AgentProvider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Persisted {
    pub revision: u64,
    pub harnesses: Vec<PersistedHarness>,
    pub contexts: Vec<PersistedContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedHarness {
    pub id: AgentProvider,
    pub installation: InstallationState,
    pub installed_version: Option<String>,
    pub checked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedContext {
    pub id: String,
    #[serde(flatten)]
    pub facts: AuthFacts,
    pub checked_at_ms: Option<u64>,
    pub credential_generation: u64,
}

pub(super) fn load(path: &Path) -> Option<Persisted> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .map_err(|error| eprintln!("harness inventory: ignoring {}: {error}", path.display()))
        .ok()
}

/// Replace the file whole, owner-only, so a crash leaves the old one or the
/// new one and never half of either.
pub(super) fn save(path: &Path, persisted: &Persisted) {
    if let Err(error) = write(path, persisted) {
        eprintln!("harness inventory: cannot save {}: {error}", path.display());
    }
}

fn write(path: &Path, persisted: &Persisted) -> std::io::Result<()> {
    let staged = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staged)?;
    file.write_all(&serde_json::to_vec_pretty(persisted)?)?;
    file.sync_all()?;
    std::fs::rename(&staged, path)
}
