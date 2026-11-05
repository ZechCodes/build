//! User-only sealed upload admission. Value and file work runs off the app lock.
use super::{AppState, DeferredWork};
use crate::api::v1::secrets::{CreateParams, RevokeParams, SecretMetadata, SecretScope};
use crate::api::v1::{answer, Answer};
use crate::api::ApiError;
use crate::execution::CallContext;
use crate::secrets::upload::{UploadContext, UploadOpener};
use crate::secrets::{GrantScope, StageContext};
use serde_json::Value;
use std::sync::Arc;

impl AppState {
    pub fn with_secret_upload_opener(mut self, opener: Arc<dyn UploadOpener>) -> Self {
        self.secret_upload_opener = Some(opener);
        self
    }

    pub(in crate::app) fn secret_user_context(
        &self,
        context: &CallContext,
    ) -> Result<(String, String), String> {
        if !context.execution_is_live() {
            return Err("unauthorized".into());
        }
        let service = self.secret_service.as_ref().ok_or("secret_unavailable")?;
        // Both trusted adapters authorize the device owner as device_user.
        // Session/opening ids identify a connection, never the durable creator.
        Ok((service.device_id().to_string(), "device_user".into()))
    }

    pub(in crate::app) fn authorize_secret_scope(
        &self,
        scope: &SecretScope,
    ) -> Result<(), ApiError> {
        scope
            .validate()
            .map_err(|error| ApiError::classify(error.to_string()))?;
        if self.projects.get(&scope.project_id).is_none() {
            return Err(ApiError::not_found("secret_unavailable"));
        }
        if let Some(stage) = &scope.stage {
            self.authorize_secret_stage(stage, &scope.project_id)?;
        }
        Ok(())
    }

    fn secret_stage_project(&self, stage: &StageContext) -> Result<String, ApiError> {
        self.project_of(&stage.owner_id)
            .or_else(|_| {
                self.projects
                    .get(&stage.owner_id)
                    .map(|p| p.id.clone())
                    .ok_or_else(|| "secret_unavailable".to_string())
            })
            .map_err(|_| ApiError::not_found("secret_unavailable"))
    }

    fn authorize_secret_stage(&self, stage: &StageContext, project: &str) -> Result<(), ApiError> {
        if self.secret_stage_project(stage)? != project {
            return Err(ApiError::not_found("secret_unavailable"));
        }
        if let Some(draft) = &stage.draft_id {
            canonical_uuid(draft)?;
            return Ok(());
        }
        let address = self
            .resolve_conversation_address(&stage.owner_id, Some(&stage.conversation_id))
            .map_err(|_| ApiError::not_found("secret_unavailable"))?;
        let thread = self
            .conversation_at(&address)
            .map_err(|_| ApiError::not_found("secret_unavailable"))?;
        if address.conversation_id != stage.conversation_id || thread.id != stage.generation {
            return Err(ApiError::conflict("secret_conflict", None));
        }
        Ok(())
    }

    fn authorize_secret_upload(&self, params: &CreateParams) -> Result<(), ApiError> {
        let project = self.secret_stage_project(&params.scope)?;
        self.authorize_secret_scope(&SecretScope {
            project_id: project.clone(),
            stage: Some(params.scope.clone()),
        })?;
        let valid = match &params.grant {
            GrantScope::Project { project_id } => project_id == &project,
            GrantScope::Agent {
                agent_id,
                conversation_id,
                generation,
            } => {
                let same = conversation_id == &params.scope.conversation_id
                    && generation == &params.scope.generation;
                if params.scope.draft_id.is_some() {
                    same && !agent_id.is_empty()
                } else {
                    same && self
                        .resolve_conversation_address(&params.scope.owner_id, Some(agent_id))
                        .is_ok_and(|address| address.conversation_id == *conversation_id)
                }
            }
        };
        if !valid {
            return Err(ApiError::invalid_params("secret_invalid"));
        }
        Ok(())
    }

    pub(crate) fn secret_create(
        &mut self,
        context: Option<&CallContext>,
        params: CreateParams,
    ) -> Result<Answer<SecretMetadata>, ApiError> {
        let caller = context.ok_or_else(|| ApiError::unavailable("unauthorized"))?;
        let (device, creator) = self
            .secret_user_context(caller)
            .map_err(ApiError::classify)?;
        canonical_uuid(&params.request_id)?;
        if params.device_id != device || params.creator != creator {
            return Err(ApiError::unavailable("unauthorized"));
        }
        validate_create_size(&params)?;
        self.authorize_secret_upload(&params)?;
        let authority = SecretScope {
            project_id: self.secret_stage_project(&params.scope)?,
            stage: Some(params.scope.clone()),
        };
        let expected = UploadContext {
            device_id: device,
            creator,
            request_id: params.request_id,
            name: params.name,
            ttl_seconds: params.ttl_seconds,
            grant: params.grant,
            stage: params.scope,
        };
        expected
            .validate()
            .map_err(|error| ApiError::classify(error.to_string()))?;
        let service = self
            .secret_service
            .clone()
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))?;
        let opener = self
            .secret_upload_opener
            .clone()
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))?;
        let caller = caller.clone();
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            if !caller.execution_is_live() {
                return Err("unauthorized".into());
            }
            let record = service
                .stage_upload_authorized(
                    &expected,
                    &params.sealed_payload,
                    opener.as_ref(),
                    &authority,
                )
                .map_err(|error| error.to_string())?;
            serde_json::to_value(SecretMetadata::from_record(
                &record,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            ))
            .map_err(|_| "secret_unavailable".into())
        })));
        answer(Ok(Value::Null))
    }

    pub(crate) fn secret_revoke(
        &mut self,
        context: Option<&CallContext>,
        params: RevokeParams,
    ) -> Result<Answer<SecretMetadata>, ApiError> {
        let caller = context.ok_or_else(|| ApiError::unavailable("unauthorized"))?;
        let (device, creator) = self
            .secret_user_context(caller)
            .map_err(ApiError::classify)?;
        canonical_uuid(&params.operation_id)?;
        self.authorize_secret_scope(&params.scope)?;
        let writer = self
            .store
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))?
            .writer();
        let service = self
            .secret_service
            .clone()
            .ok_or_else(|| ApiError::unavailable("secret_unavailable"))?;
        let caller = caller.clone();
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            if !caller.execution_is_live() {
                return Err("unauthorized".into());
            }
            let query = crate::api::v1::secrets::ListParams {
                scope: params.scope,
                secret_ids: vec![params.secret_id.clone()],
                request_ids: vec![],
                after: None,
                limit: 1,
            };
            let mut metadata = writer
                .secret_metadata_page(&device, &creator, &query)
                .map_err(|error| error.to_string())?
                .secrets
                .into_iter()
                .next()
                .ok_or("secret_unavailable")?;
            let record = service
                .revoke(&params.secret_id)
                .map_err(|error| error.to_string())?;
            metadata.revoked_at = record.revoked_at;
            metadata.revision = record.revision;
            metadata.status = "revoked".into();
            serde_json::to_value(metadata).map_err(|_| "secret_unavailable".into())
        })));
        answer(Ok(Value::Null))
    }
}

fn validate_create_size(params: &CreateParams) -> Result<(), ApiError> {
    let size = serde_json::to_vec(params)
        .map_err(|_| ApiError::invalid_params("secret_invalid"))?
        .len();
    if size > crate::secrets::upload::MAX_CREATE_REQUEST_BYTES {
        return Err(ApiError::invalid_params("secret_invalid"));
    }
    Ok(())
}

pub(in crate::app) fn canonical_uuid(value: &str) -> Result<(), ApiError> {
    if uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value) {
        Ok(())
    } else {
        Err(ApiError::invalid_params("secret_invalid"))
    }
}

#[cfg(test)]
mod tests;
