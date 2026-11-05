//! Sealed uploads and bounded, metadata-only secret operations.
use super::{Answer, Handler};
use crate::api::ApiError;
use crate::app::AppState;
use crate::execution::CallContext;
use crate::secrets::{GrantScope, SecretRecord, StageContext};
use crate::{v1_context_method, v1_methods};
use serde::{Deserialize, Serialize};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_context_method!("secrets.create", create, CreateParams, SecretMetadata),
        v1_context_method!("secrets.list", list, ListParams, MetadataPage),
        v1_context_method!("secrets.revoke", revoke, RevokeParams, SecretMetadata),
        v1_context_method!("secrets.audit", audit, AuditParams, AuditPage),
    ]
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretScope {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<StageContext>,
}

impl SecretScope {
    pub fn validate(&self) -> crate::secrets::Result<()> {
        let mut fields = vec![self.project_id.as_str()];
        if let Some(stage) = &self.stage {
            fields.extend([
                stage.owner_id.as_str(),
                stage.conversation_id.as_str(),
                stage.generation.as_str(),
            ]);
            if let Some(draft) = &stage.draft_id {
                let id = uuid::Uuid::parse_str(draft)
                    .map_err(|_| crate::secrets::SecretError::Invalid)?;
                if id.hyphenated().to_string() != *draft {
                    return Err(crate::secrets::SecretError::Invalid);
                }
            }
        }
        if fields
            .iter()
            .any(|field| field.is_empty() || field.len() > 128 || field.contains('\0'))
        {
            return Err(crate::secrets::SecretError::Invalid);
        }
        Ok(())
    }
}

// Upload bytes deliberately have no Debug implementation.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    pub request_id: String,
    pub device_id: String,
    pub creator: String,
    pub scope: StageContext,
    pub grant: GrantScope,
    pub name: String,
    pub ttl_seconds: Option<i64>,
    pub sealed_payload: String,
}

pub fn default_limit() -> usize {
    100
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    pub scope: SecretScope,
    #[serde(default)]
    pub secret_ids: Vec<String>,
    #[serde(default)]
    pub request_ids: Vec<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeParams {
    pub scope: SecretScope,
    pub secret_id: String,
    pub operation_id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditParams {
    pub scope: SecretScope,
    pub secret_id: String,
    #[serde(default)]
    pub after: i64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Explicit allowlist: storage blob facts and ciphertext digests never escape.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretMetadata {
    pub secret_id: String,
    pub request_id: String,
    pub name: String,
    pub ttl_seconds: Option<i64>,
    pub grant: GrantScope,
    pub scope: StageContext,
    pub created_at: i64,
    pub attached_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revision: i64,
    pub status: String,
}

impl SecretMetadata {
    pub fn from_record(record: &SecretRecord, now: i64) -> Self {
        let status = if record.revocation_reason.as_deref() == Some("expired") {
            "expired"
        } else if record.revoked_at.is_some() {
            "revoked"
        } else if record.expires_at.is_some_and(|expiry| now >= expiry)
            || (record.attached_at.is_none()
                && now.saturating_sub(record.created_at) >= crate::secrets::STAGED_LIFETIME_SECONDS)
        {
            "expired"
        } else if !record.blob_present {
            "unavailable"
        } else if record.attached_at.is_none() {
            "staged"
        } else {
            "available"
        };
        Self {
            secret_id: record.context.id.clone(),
            request_id: record.upload_request_id.clone(),
            name: record.context.name.clone(),
            ttl_seconds: record.context.ttl_seconds,
            grant: record.context.grant.clone(),
            scope: record.context.stage.clone(),
            created_at: record.created_at,
            attached_at: record.attached_at,
            expires_at: record.expires_at,
            revoked_at: record.revoked_at,
            revision: record.revision,
            status: status.into(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct MetadataPage {
    pub secrets: Vec<SecretMetadata>,
    pub server_now: i64,
    pub next: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct AuditPage {
    pub events: Vec<crate::secrets::SecretAuditRecord>,
    pub server_now: i64,
    pub next: Option<i64>,
}

fn create(
    app: &mut AppState,
    context: &CallContext,
    params: CreateParams,
) -> Result<Answer<SecretMetadata>, ApiError> {
    app.secret_create(Some(context), params)
}
fn revoke(
    app: &mut AppState,
    context: &CallContext,
    params: RevokeParams,
) -> Result<Answer<SecretMetadata>, ApiError> {
    app.secret_revoke(Some(context), params)
}
fn list(
    app: &mut AppState,
    context: &CallContext,
    params: ListParams,
) -> Result<Answer<MetadataPage>, ApiError> {
    app.secret_list(Some(context), params)
}
fn audit(
    app: &mut AppState,
    context: &CallContext,
    params: AuditParams,
) -> Result<Answer<AuditPage>, ApiError> {
    app.secret_audit(Some(context), params)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swept_expiry_tombstone_remains_expired_metadata() {
        let context = crate::secrets::SecretContext {
            id: format!("sec_{}", uuid::Uuid::new_v4()),
            device_id: "synthetic-device".into(),
            creator: "device_user".into(),
            name: "TEST".into(),
            ttl_seconds: Some(60),
            grant: GrantScope::Project {
                project_id: "synthetic-project".into(),
            },
            stage: StageContext {
                owner_id: "owner".into(),
                conversation_id: "agent".into(),
                generation: "thread".into(),
                draft_id: None,
            },
        };
        let record = SecretRecord {
            context,
            created_at: 100,
            attached_at: Some(100),
            expires_at: Some(160),
            revoked_at: Some(160),
            revocation_reason: Some("expired".into()),
            revision: 3,
            message_operation_id: Some("operation".into()),
            upload_request_id: uuid::Uuid::new_v4().to_string(),
            upload_digest: "a".repeat(64),
            blob_present: false,
        };
        assert_eq!(SecretMetadata::from_record(&record, 160).status, "expired");
    }
    #[test]
    fn secret_contract_fixtures_round_trip() {
        for (method, _) in methods() {
            super::super::testing::fixture_round_trips(methods(), method);
        }
    }
}
