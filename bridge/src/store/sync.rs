//! Durable scoped publication, independent from command receipts and timelines.
//!
//! Producers collect bounded summary-visible facts in PublicationBatch and
//! publish on their canonical transaction. This module never consumes ChangeBus.

mod continuations;
mod cursor;
pub use continuations::Continuation;
mod journal;
mod reads;
mod recovery_reads;
mod retention;
mod schema;
mod task_publication;
pub(super) use task_publication::{collect as collect_tasks, touch as touch_task};

pub use journal::PublicationBatch;
pub use reads::{Head, HeadResult, SyncReadConnection};
pub use retention::{
    GcProgress, GC_ROWS, MAX_JOURNAL_AGE_MS, MAX_JOURNAL_BYTES, MAX_JOURNAL_ENTRIES,
};
pub(super) use schema::{initialize, validate_startup};

use super::StoreError;
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
        }
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
mod startup_tests;
#[cfg(test)]
mod task_producer_tests;
#[cfg(test)]
mod tests;
