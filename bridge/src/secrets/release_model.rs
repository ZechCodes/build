//! Canonical identity derived from a purpose-limited issued session, never from
//! a release request's owner/project/CWD selectors.
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq)]
pub struct SecretCaller {
    pub device_id: String,
    pub channel: String,
    pub project_id: String,
    pub owner_id: String,
    pub canonical_owner_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub thread_id: String,
    /// The immutable thread ID, matching GrantScope::Agent::generation.
    pub generation: String,
    pub generation_revision: u64,
    pub session_incarnation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SecretAuditRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    pub sequence: i64,
    pub secret_id: String,
    pub creator: Option<String>,
    pub project_id: String,
    pub owner_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub generation: String,
    pub session_incarnation: String,
    /// Bridge-generated audit identity, independent of the client request ID.
    pub request_id: String,
    pub occurred_at: i64,
    /// Coalesced attempts for denials; every release has count one.
    pub attempt_count: i64,
    pub last_occurred_at: i64,
    pub outcome: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SecretAuditSummary {
    pub release_count: i64,
    pub denial_count: i64,
    pub last_released_at: Option<i64>,
    pub last_outcome: String,
}
