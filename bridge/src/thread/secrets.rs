//! The allowlisted metadata retained with a secret-bearing message.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageSecretScope {
    Agent,
    Project,
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretEnvironment {
    Dev,
    Prod,
}

impl SecretEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "prod",
        }
    }
}

impl From<crate::config::Channel> for SecretEnvironment {
    fn from(channel: crate::config::Channel) -> Self {
        match channel {
            crate::config::Channel::Dev => Self::Dev,
            crate::config::Channel::Prod => Self::Prod,
        }
    }
}

/// Deliberately has no value, upload envelope or filesystem locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSecret {
    pub name: String,
    pub secret_id: String,
    pub scope: MessageSecretScope,
    pub ttl_seconds: Option<i64>,
    pub expires_at: Option<String>,
}

impl MessageSecret {
    pub fn from_record(record: &crate::secrets::SecretRecord) -> crate::secrets::Result<Self> {
        let expires_at = record
            .expires_at
            .map(|timestamp| {
                time::OffsetDateTime::from_unix_timestamp(timestamp)
                    .map_err(|_| crate::secrets::SecretError::Invalid)?
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|_| crate::secrets::SecretError::Invalid)
            })
            .transpose()?;
        Ok(Self {
            name: record.context.name.clone(),
            secret_id: record.context.id.clone(),
            scope: match record.context.grant {
                crate::secrets::GrantScope::Agent { .. } => MessageSecretScope::Agent,
                crate::secrets::GrantScope::Project { .. } => MessageSecretScope::Project,
                crate::secrets::GrantScope::Environment { .. } => MessageSecretScope::Environment,
            },
            ttl_seconds: record.context.ttl_seconds,
            expires_at,
        })
    }
}
