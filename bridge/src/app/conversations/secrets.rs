//! Secret-bearing posts enter only through the authenticated user facade.
use super::{inputs::PostOrigin, AppState, ConversationAddress};
use crate::operation::OperationPayload;
use crate::secrets::SecretPostBinding;
use crate::thread::{MessageSecret, SecretEnvironment, Thread, ThreadItem};
use serde_json::Value;
use std::collections::HashSet;

pub(super) fn secret_ids(params: &Value) -> Result<Vec<String>, String> {
    let Some(value) = params.get("secret_ids") else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or("secret_ids must be an array")?;
    if values.len() > 10 {
        return Err("secret_ids must contain at most 10 entries".into());
    }
    let ids = values
        .iter()
        .map(|value| {
            let id = value.as_str().ok_or("every secret_id must be a string")?;
            crate::secrets::validate_id(id).map_err(|error| error.to_string())?;
            Ok(id.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err("secret_ids must be distinct".into());
    }
    Ok(ids)
}

pub(super) fn require_secret_user(params: &Value, origin: &PostOrigin) -> Result<(), String> {
    if !secret_ids(params)?.is_empty()
        && (origin.authenticated_user.is_none()
            || origin.from_agent.is_some()
            || origin.requested_by.is_some()
            || origin.from_task.is_some())
    {
        return Err("only the authenticated user may bind secrets".into());
    }
    Ok(())
}

impl AppState {
    pub(super) fn prepare_post_secrets(
        &self,
        params: &Value,
        address: &ConversationAddress,
        origin: &PostOrigin,
    ) -> Result<Option<(SecretPostBinding, Vec<MessageSecret>)>, String> {
        let ids = secret_ids(params)?;
        if ids.is_empty() {
            return Ok(None);
        }
        if origin.from_agent.is_some()
            || origin.requested_by.is_some()
            || origin.from_task.is_some()
        {
            return Err("only the authenticated user may bind secrets".into());
        }
        let (device_id, creator) = origin
            .authenticated_user
            .as_ref()
            .ok_or("only the authenticated user may bind secrets")?;
        let thread = self.conversation_at(address)?;
        validate_secret_request(params, &thread.id, origin)?;
        self.secret_environment()
            .ok_or("secret environment unavailable")?;
        let draft_id = params
            .get("draft_id")
            .map(|value| {
                value
                    .as_str()
                    .ok_or("draft_id must be a string")
                    .map(str::to_string)
            })
            .transpose()?;
        let claim = SecretPostBinding {
            ids,
            device_id: device_id.clone(),
            creator: creator.clone(),
            project_id: self.project_of(&address.entity_id)?,
            owner_id: address.entity_id.clone(),
            agent_id: address.agent_id.clone(),
            conversation_id: address.conversation_id.clone(),
            generation: thread.id.clone(),
            draft_id,
            attached_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        };
        let store = self.require_store()?;
        let records = store
            .writer()
            .secrets_post_preflight(&claim)
            .map_err(|error| error.to_string())?;
        let metadata = records
            .iter()
            .map(MessageSecret::from_record)
            .collect::<crate::secrets::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
        let provider = self
            .entity_agents(&address.entity_id)?
            .turn_choice(&address.agent_id)?
            .provider;
        refuse_provider_command(params, provider)?;
        Ok(Some((claim, metadata)))
    }

    pub(in crate::app) fn notify_bound_post_secrets(&self, binding: Option<&SecretPostBinding>) {
        let Some(binding) = binding else {
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let mut records = Vec::new();
        for id in &binding.ids {
            let Ok(Some(mut record)) = store.writer().secret_record(id) else {
                continue;
            };
            // A metadata-only projection; the encrypted AAD is never changed.
            record.context.stage.owner_id = binding.owner_id.clone();
            record.context.stage.conversation_id = binding.conversation_id.clone();
            record.context.stage.generation = binding.generation.clone();
            if matches!(
                record.context.grant,
                crate::secrets::GrantScope::Agent { .. }
            ) {
                record.context.grant = crate::secrets::GrantScope::Agent {
                    agent_id: binding.agent_id.clone(),
                    conversation_id: binding.conversation_id.clone(),
                    generation: binding.generation.clone(),
                };
            }
            records.push(record);
        }
        self.changes.note_secret_records(&records);
    }

    pub(super) fn secret_environment(&self) -> Option<SecretEnvironment> {
        self.session_registry
            .secret_launch_context()
            .and_then(|(_, channel, _)| match channel {
                "dev" => Some(SecretEnvironment::Dev),
                "prod" => Some(SecretEnvironment::Prod),
                _ => None,
            })
    }
}

pub(super) fn attach_post_metadata(
    thread: &mut Thread,
    payload: &mut Option<OperationPayload>,
    origin: &PostOrigin,
    environment: Option<SecretEnvironment>,
) -> Result<(), String> {
    if origin.secret_metadata.is_empty() {
        return Ok(());
    }
    let environment = environment.ok_or("secret environment unavailable")?;
    let Some(ThreadItem::Message(message)) = thread.items.last_mut() else {
        return Err("secret message missing".into());
    };
    message.secrets = origin.secret_metadata.clone();
    let payload = payload.as_mut().ok_or("secret delivery payload missing")?;
    let message = payload
        .messages
        .last_mut()
        .ok_or("secret message missing")?;
    message.secrets = origin.secret_metadata.clone();
    payload.secret_environment = Some(environment);
    Ok(())
}

fn validate_secret_request(
    params: &Value,
    generation: &str,
    origin: &PostOrigin,
) -> Result<(), String> {
    if params.get("thread_id").and_then(Value::as_str) != Some(generation) {
        return Err("secret-bearing thread.post requires the current thread_id".into());
    }
    if origin.operation_id.is_none() {
        return Err("secret-bearing thread.post requires operation_id".into());
    }
    if params.get("choice").is_some() || params.get("option_choice").is_some() {
        return Err("secret references cannot accompany an option choice".into());
    }
    Ok(())
}

fn refuse_provider_command(
    params: &Value,
    provider: crate::models::AgentProvider,
) -> Result<(), String> {
    let body = match params.get("messages").and_then(Value::as_array) {
        Some(messages) => {
            let [message] = messages.as_slice() else {
                return Ok(());
            };
            message.get("body").and_then(Value::as_str).unwrap_or("")
        }
        None => params.get("body").and_then(Value::as_str).unwrap_or(""),
    };
    if crate::harness::harness_for(provider).requires_unadorned_command(body.trim())
        || crate::harness::unsupported_slash_command(provider, body.trim()).is_some()
    {
        return Err("provider commands cannot carry secret references".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_commands_are_refused_only_for_a_single_actual_provider_command() {
        let provider = crate::models::AgentProvider::Claude;
        assert!(refuse_provider_command(&json!({"body":"/compact"}), provider).is_err());
        assert!(
            refuse_provider_command(&json!({"body":"/api/v1/users returns 500"}), provider).is_ok()
        );
        assert!(refuse_provider_command(
            &json!({"messages":[{"body":"/compact"},{"body":"explain the command"}]}),
            provider
        )
        .is_ok());
        assert!(
            refuse_provider_command(&json!({"messages":[{"body":"/clear"}]}), provider).is_err()
        );
    }
}
