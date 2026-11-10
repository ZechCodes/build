use super::{Result, SecretError};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_VALUE_BYTES: usize = 16 * 1024;
pub const MAX_STAGED_PER_CREATOR: usize = 100;
pub const MAX_RETAINED_BLOBS: usize = 1_000;
pub const MAX_CONCURRENT_JOBS: usize = 4;
pub const STAGED_LIFETIME_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum GrantScope {
    Agent {
        agent_id: String,
        conversation_id: String,
        generation: String,
    },
    Project {
        project_id: String,
    },
    Environment {
        environment_id: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageContext {
    pub owner_id: String,
    pub conversation_id: String,
    pub generation: String,
    pub draft_id: Option<String>,
}

/// Immutable context authenticated by every vault blob. Metadata only.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretContext {
    pub id: String,
    pub device_id: String,
    pub creator: String,
    pub name: String,
    pub ttl_seconds: Option<i64>,
    pub grant: GrantScope,
    pub stage: StageContext,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRecord {
    pub context: SecretContext,
    pub created_at: i64,
    pub attached_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revocation_reason: Option<String>,
    pub revision: i64,
    pub message_operation_id: Option<String>,
    pub upload_request_id: String,
    /// Digest of the sealed upload bytes, never of the plaintext.
    pub upload_digest: String,
    pub blob_present: bool,
}

// Names and arbitrary caller metadata can be sensitive even when a value is
// absent. Debug formatting never traverses those fields, including nested ones.
impl fmt::Debug for StageContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StageContext")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for SecretContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretContext")
            .field("name", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for SecretRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretRecord")
            .field("context", &self.context)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// Intentionally neither Debug nor Serialize. Borrow only for encryption/release.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let value = Self(bytes);
        if value.0.len() > MAX_VALUE_BYTES || value.0.contains(&0) {
            return Err(SecretError::Invalid);
        }
        Ok(value)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        // zeroize prevents the compiler from eliding this buffer cleanup.
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

pub fn validate_id(id: &str) -> Result<()> {
    let raw = id.strip_prefix("sec_").ok_or(SecretError::Invalid)?;
    let uuid = uuid::Uuid::parse_str(raw).map_err(|_| SecretError::Invalid)?;
    if uuid.hyphenated().to_string() != raw {
        return Err(SecretError::Invalid);
    }
    Ok(())
}

impl SecretContext {
    pub fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        for text in [
            &self.device_id,
            &self.creator,
            &self.name,
            &self.stage.owner_id,
            &self.stage.conversation_id,
            &self.stage.generation,
        ] {
            validate_text(text)?;
        }
        if let Some(draft) = &self.stage.draft_id {
            validate_text(draft)?;
        }
        if self.ttl_seconds.is_some_and(|ttl| ttl <= 0) {
            return Err(SecretError::Invalid);
        }
        match &self.grant {
            GrantScope::Agent {
                agent_id,
                conversation_id,
                generation,
            } => {
                for text in [agent_id, conversation_id, generation] {
                    validate_text(text)?;
                }
            }
            GrantScope::Project { project_id } => validate_text(project_id)?,
            GrantScope::Environment { environment_id } => validate_text(environment_id)?,
        }
        Ok(())
    }
}

fn validate_text(text: &str) -> Result<()> {
    if text.is_empty() || text.len() > 128 || text.contains('\0') {
        return Err(SecretError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
