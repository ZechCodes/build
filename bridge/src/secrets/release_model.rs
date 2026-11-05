//! Canonical identity derived from a purpose-limited issued session, never from
//! a release request's owner/project/CWD selectors.
use serde::Serialize;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretAuditRecord {
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
    pub outcome: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SecretAuditSummary {
    pub release_count: i64,
    pub denial_count: i64,
    pub last_released_at: Option<i64>,
    pub last_outcome: String,
}
