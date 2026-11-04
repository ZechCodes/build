//! Durable scoped publication, independent from command receipts and timelines.
//!
//! Producers collect bounded summary-visible facts in PublicationBatch and
//! publish on their canonical transaction. This module never consumes ChangeBus.

mod continuations;
mod cursor;
pub use continuations::Continuation;
mod device;
pub(in crate::store) use device::stage as stage_device;
pub use device::{DeviceLease, DEVICE_LEASES_PER_OPENING, DEVICE_LEASE_TTL_MS};
mod device_334_cleanup;
mod device_backfill;
mod device_facts;
mod device_families;
mod device_producers;
mod device_session_backfill;
pub(in crate::store) use device_sessions::reset as reset_device_sessions;
mod device_sessions;
mod device_thread;
pub(in crate::store) use device_facts::{
    forget_agent as device_facts_forget_agent, seed_agent as seed_device_agent,
    seed_owner as seed_device_owner, seed_pin as seed_device_pin,
};
pub(in crate::store) use device_families::{
    clear_project as clear_device_project, forget_owner as forget_device_owner,
    lifecycle as stage_device_lifecycle, retain_agent as retain_device_agent,
    retain_owner as retain_device_owner, touch_archive as touch_device_archive,
};
pub(in crate::store) use device_producers::{
    touch_agent as touch_device_agent, touch_conversation as touch_device_conversation,
    touch_owner as touch_device_owner, touch_project as touch_device_project,
};
pub(in crate::store) use device_thread::{
    before_item as before_device_item, reset as reset_device_conversation,
};
mod journal;
mod lifecycle;
mod reads;
mod recovery_reads;
mod retention;
mod retention_install;
mod schema;
mod task_publication;
pub(super) use device_producers::collect as collect_device;
pub(super) use task_publication::{collect as collect_tasks, touch as touch_task};

pub use journal::PublicationBatch;
pub use reads::{Head, HeadResult, SyncReadConnection};
pub use recovery_reads::CursorSequence;
pub use retention::{
    GcProgress, GC_ROWS, MAX_JOURNAL_AGE_MS, MAX_JOURNAL_BYTES, MAX_JOURNAL_ENTRIES,
};
pub const MAX_RECOVERY_EVENTS: usize = 500;
pub const MAX_RECOVERY_BYTES: usize = 1024 * 1024;
pub(super) use schema::{initialize, validate_startup};

use super::StoreError;
use aws_lc_rs::hmac;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// SQLite's exact nonnegative integer domain, never a JavaScript number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sequence(i64);

impl Sequence {
    pub const ZERO: Self = Self(0);
    pub fn new(value: i64) -> Result<Self, StoreError> {
        if value < 0 {
            return Err(invalid("negative sequence"));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> i64 {
        self.0
    }
}
impl TryFrom<String> for Sequence {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let sequence = value.parse::<i64>().map_err(|_| "invalid sync sequence")?;
        if sequence < 0 || sequence.to_string() != value {
            return Err("invalid sync sequence");
        }
        Ok(Self(sequence))
    }
}
impl From<Sequence> for String {
    fn from(value: Sequence) -> Self {
        value.0.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Device,
    ProjectTasks,
    Task,
}
impl StreamKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::ProjectTasks => "project_tasks",
            Self::Task => "task",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub kind: StreamKind,
    pub key: String,
}
impl Stream {
    pub fn device() -> Self {
        Self {
            kind: StreamKind::Device,
            key: String::new(),
        }
    }
    pub fn project(key: &str) -> Self {
        Self {
            kind: StreamKind::ProjectTasks,
            key: key.into(),
        }
    }
    pub fn task(key: &str) -> Self {
        Self {
            kind: StreamKind::Task,
            key: key.into(),
        }
    }
}

/// Construct only after validating authority and normalizing task selectors.
/// Equivalent selection sets must be canonicalized by the shared query parser;
/// opaque actor bytes are preserved. No client-provided principal is accepted.
pub struct Scope {
    stream: Stream,
    digest: String,
    binding_path: Option<String>,
    requires_path_binding: bool,
}
impl Scope {
    pub fn new(stream: Stream, normalized_definition: Value) -> Self {
        let bytes = json!([
            "sync-scope",
            1,
            "device_user",
            stream,
            normalized_definition
        ])
        .to_string();
        Self {
            stream,
            digest: format!("{:x}", Sha256::digest(bytes.as_bytes())),
            binding_path: None,
            requires_path_binding: false,
        }
    }
    /// Route with a local durable key while authenticating only the stable
    /// wire identity supplied by the client.
    pub fn wire_bound(
        stream: Stream,
        normalized_definition: Value,
        binding_path: Option<&str>,
    ) -> Self {
        let bytes = json!([
            "sync-scope",
            1,
            "device_user",
            stream.kind,
            normalized_definition
        ])
        .to_string();
        Self {
            stream,
            digest: format!("{:x}", Sha256::digest(bytes.as_bytes())),
            binding_path: binding_path.map(str::to_owned),
            requires_path_binding: true,
        }
    }
    pub(super) fn path_binding(&self, key: &[u8]) -> Option<String> {
        self.binding_path.as_ref().map(|path| {
            let key = hmac::Key::new(hmac::HMAC_SHA256, key);
            let mut input = b"project-path\0".to_vec();
            input.extend_from_slice(path.as_bytes());
            URL_SAFE_NO_PAD.encode(hmac::sign(&key, &input).as_ref())
        })
    }
    pub(super) fn path_binding_required(&self) -> bool {
        self.requires_path_binding
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn stream(&self) -> &Stream {
        &self.stream
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entity {
    pub kind: String,
    pub key: String,
}
impl Entity {
    pub fn new(kind: &str, key: &str) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Upsert,
    Delete,
}
impl Operation {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: Sequence,
    pub entity: Entity,
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    IncarnationChanged,
    ScopeRemoved,
    RetentionExpired,
    OverBudget,
    LeaseExpired,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("invalid sync request: {0}")]
    InvalidRequest(String),
    #[error("sync data unavailable: {0}")]
    Unavailable(String),
    #[error("malformed sync cursor")]
    MalformedCursor,
    #[error("sync cursor belongs to a different scope")]
    ScopeMismatch,
    #[error("sync cursor is ahead of the journal head")]
    FutureCursor,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("sync read: {0}")]
    Database(#[from] rusqlite::Error),
}

fn invalid(message: &str) -> StoreError {
    StoreError::SyncInvalid(message.into())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod cursor_tests;
#[cfg(test)]
mod device_producer_tests;
#[cfg(test)]
mod device_tests;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
mod task_producer_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod device_entry_tests;

#[cfg(test)]
mod device_session_tests;
