//! One durable command owns the project and its first unpublished workspace.
use super::AppState;
use crate::api::writes::{ProgressState, WriteMetadata, WriteProgress};
use crate::api::ApiError;
use crate::store::{
    MutationAdmission, MutationOwner, MutationRecord, MutationResource, StoreWriter,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub(in crate::app) const PRINCIPAL: &str = "device_user";
mod execution;
mod service;
pub(in crate::app) use execution::execute;
#[cfg(test)]
use execution::project_step;
pub(in crate::app) use service::{dispatch, start};
#[cfg(test)]
mod tests;

pub(in crate::app) fn admit(
    state: &Arc<Mutex<AppState>>,
    method: &str,
    params: Value,
    metadata: WriteMetadata,
) -> Result<MutationRecord, ApiError> {
    let params = crate::api::v1::normalize_read_params(method, &params)?;
    validate_creation(method, &params)?;
    let writer = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("creation needs a durable store"))?
        .writer();
    if let Some(record) = writer
        .creation_record(PRINCIPAL, &metadata.operation_id)
        .map_err(admission_error)?
    {
        return retry_recorded(&writer, record, method, params, metadata);
    }
    let CreationScope {
        writer,
        scope,
        owner,
        project_key,
    } = capture_scope(&state.lock().unwrap(), method, &params)?;
    validate_continuation(&writer, &params, &scope, project_key.as_deref())?;
    let (acceptance, reservations) = reserve_attempt(state, method, &params, &owner)?;
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::AdmissionPrepared);
    writer
        .admit_mutation(&MutationAdmission {
            principal: PRINCIPAL.into(),
            operation_id: metadata.operation_id,
            method: method.into(),
            params,
            scope,
            owners: vec![owner],
            resources: vec![],
            reservations,
            after: metadata.after,
            recovery_policy: "external".into(),
            accepted_epoch: crate::bridge_epoch::current().into(),
            acceptance,
        })
        .map_err(admission_error)
}
fn retry_recorded(
    writer: &StoreWriter,
    record: MutationRecord,
    method: &str,
    params: Value,
    metadata: WriteMetadata,
) -> Result<MutationRecord, ApiError> {
    // The paired-device principal can read its retained result even when the
    // original scope is gone. Reuse its saved authority and reservations, but
    // let the journal compare the submitted method, intent and dependencies.
    writer
        .admit_mutation(&MutationAdmission {
            principal: PRINCIPAL.into(),
            operation_id: metadata.operation_id,
            method: method.into(),
            params,
            scope: record.scope,
            owners: record
                .owners
                .into_iter()
                .map(|owner| MutationOwner {
                    kind: owner.kind,
                    id: owner.id,
                    generation: owner.generation,
                })
                .collect(),
            resources: record.resources,
            reservations: record.reservations,
            after: metadata.after,
            recovery_policy: record.recovery_policy,
            accepted_epoch: record.accepted_epoch,
            acceptance: record.acceptance,
        })
        .map_err(admission_error)
}

struct CreationScope {
    writer: StoreWriter,
    scope: Value,
    owner: MutationOwner,
    project_key: Option<String>,
}
fn capture_scope(app: &AppState, method: &str, params: &Value) -> Result<CreationScope, ApiError> {
    if app.project_deletion_in_progress {
        return Err(ApiError::busy(super::rpc::PROJECT_DELETION_IN_PROGRESS));
    }
    let store = app
        .store
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("creation needs a durable store"))?;
    let (kind, id, project_key) = if method == "environment.create_project" {
        let id = params["environment_id"].as_str().unwrap();
        app.projects
            .environment(id)
            .ok_or_else(|| ApiError::invalid_params("unknown environment"))?;
        if app.config_path.is_none() {
            return Err(ApiError::unavailable(
                "creation needs durable project configuration",
            ));
        }
        ("environment", id, None)
    } else {
        let id = params["project_id"].as_str().unwrap();
        let project = app.project_for(id).map_err(ApiError::from)?;
        ("project", id, Some(project.project_key.clone()))
    };
    Ok(CreationScope {
        writer: store.writer(),
        scope: json!({format!("{kind}_id"):id}),
        owner: MutationOwner {
            kind: kind.into(),
            id: id.into(),
            generation: 0,
        },
        project_key,
    })
}
fn reserve_attempt(
    state: &Arc<Mutex<AppState>>,
    method: &str,
    params: &Value,
    owner: &MutationOwner,
) -> Result<(Value, Vec<MutationResource>), ApiError> {
    let creation = if method == "environment.create_project" {
        json!({"project_key":format!("project:{}",uuid::Uuid::new_v4()),"steps":[]})
    } else {
        let app = state.lock().unwrap();
        let project = app
            .project_for(params["project_id"].as_str().unwrap())
            .map_err(ApiError::from)?;
        let workspace = app
            .reserve_creation_workspace(params["project_id"].as_str().unwrap(), params)
            .map_err(ApiError::from)?;
        json!({"project_id":params["project_id"],"project_key":project.project_key,"workspace":workspace,"steps":[]})
    };
    let (kind, id) = if method == "environment.create_project" {
        ("project", creation["project_key"].as_str())
    } else {
        ("workspace", creation["workspace"]["id"].as_str())
    };
    let id = id.ok_or_else(|| ApiError::unavailable("creation reservation missing"))?;
    let reservation = MutationResource {
        kind: kind.into(),
        id: id.into(),
        owner: owner.clone(),
    };
    let mut creation = creation;
    creation["effect_state"] = json!("not_started");
    creation["effect_step"] = json!(if method == "environment.create_project" {
        "project"
    } else {
        "workspace"
    });
    Ok((json!({"creation":creation}), vec![reservation]))
}
fn admission_error(error: crate::store::StoreError) -> ApiError {
    match error {
        crate::store::StoreError::OperationConflict { operation_id } => ApiError::conflict(
            "A conflicting creation attempt is retained",
            Some(json!({"operation_id":operation_id})),
        ),
        crate::store::StoreError::MutationBusy => {
            ApiError::busy("creation journal capacity reached")
        }
        crate::store::StoreError::MutationInvalid(message) => ApiError::invalid_params(message),
        error => ApiError::unavailable(error.to_string()),
    }
}
fn validate_creation(method: &str, params: &Value) -> Result<(), ApiError> {
    if !matches!(method, "environment.create_project" | "workspace.create") {
        return Err(ApiError::unavailable("method is not a journaled creation"));
    }
    let workspace = if method == "environment.create_project" {
        let name = params["name"].as_str().unwrap_or("");
        if name.trim().is_empty() || name.len() > 256 {
            return Err(ApiError::invalid_params(
                "project name must be between 1 and 256 bytes",
            ));
        }
        &params["workspace"]
    } else {
        params
    };
    if let Some(isolation) = workspace.get("isolation").and_then(Value::as_str) {
        if crate::isolation::Isolation::from_wire(isolation).is_none() {
            return Err(ApiError::invalid_params(format!(
                "unknown isolation: {isolation}"
            )));
        }
    }
    Ok(())
}
fn validate_continuation(
    writer: &StoreWriter,
    params: &Value,
    scope: &Value,
    project_key: Option<&str>,
) -> Result<(), ApiError> {
    let Some(id) = params.get("continues").and_then(Value::as_str) else {
        return Ok(());
    };
    let record = writer
        .creation_record(PRINCIPAL, id)
        .map_err(|e| ApiError::unavailable(e.to_string()))?
        .ok_or_else(|| ApiError::invalid_params("continued attempt not found"))?;
    if !matches!(
        record.method.as_str(),
        "environment.create_project" | "workspace.create"
    ) {
        return Err(ApiError::invalid_params(
            "continued operation is not a creation attempt",
        ));
    }
    if !matches!(record.state.as_str(), "succeeded" | "failed")
        || record
            .outcome
            .as_ref()
            .is_some_and(|o| o["effect"] == "unknown")
    {
        return Err(ApiError::busy(
            "continued attempt is still checking or needs cleanup",
        ));
    }
    // A reused project number is not ownership evidence, even for equal scopes.
    // Environment chaining remains bound to its existing environment scope.
    let same_scope = match project_key {
        Some(key) => record.execution["project_key"].as_str() == Some(key),
        None => record.scope == *scope,
    };
    if !same_scope {
        return Err(ApiError::invalid_params(
            "continued attempt belongs to another scope",
        ));
    }
    Ok(())
}
impl AppState {
    pub(in crate::app) fn refuse_pending_creation(&self, project_id: &str) -> Result<(), String> {
        let Some(project) = self.projects.get(project_id) else {
            return Ok(());
        };
        let Some(store) = &self.store else {
            return Ok(());
        };
        let pending = store
            .writer()
            .pending_creations()
            .map_err(|e| e.to_string())?;
        if pending.iter().any(|record| {
            record.execution["project_key"].as_str() == Some(project.project_key.as_str())
        }) {
            return Err(crate::reclaim::BUSY.into());
        }
        Ok(())
    }
    pub(in crate::app) fn track_creation_workspace(&mut self, evidence: &Value) {
        if let (Some(id), Some(root)) = (
            evidence["workspace"]["id"].as_str(),
            evidence["workspace"]["root"].as_str(),
        ) {
            self.creation_roots.insert(id.into(), root.into());
        }
    }
    pub(in crate::app) fn release_creation_workspace(&mut self, evidence: &Value) {
        if let Some(id) = evidence["workspace"]["id"].as_str() {
            self.creation_roots.remove(id);
        }
    }
    pub(in crate::app) fn mutation_service(&mut self) -> super::mutations::MutationService {
        if let Some(service) = &self.mutation_service {
            return service.clone();
        }
        let service = super::mutations::MutationService::new(self);
        self.mutation_service = Some(service.clone());
        service
    }
    pub(crate) fn environment_create_project(&mut self, _params: &Value) -> Result<Value, String> {
        Err("environment.create_project requires frame write.operation_id".into())
    }
}
