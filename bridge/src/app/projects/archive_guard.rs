//! One archived-project policy shared by RPC, MCP and spawn reservation.
use crate::api::{effects, ApiError};
use crate::app::AppState;
use crate::mcp::BridgeAction;
use serde_json::{json, Value};

impl AppState {
    /// Drains exclude these keys before paging: suspended work neither retries
    /// nor starves other projects, and restore needs no outbox state rewrite.
    pub(in crate::app) fn archived_project_keys(&self) -> Vec<String> {
        self.projects
            .iter()
            .filter(|project| project.archived_at.is_some())
            .map(|project| project.project_key.clone())
            .chain(
                self.projects
                    .parked()
                    .iter()
                    .filter(|project| project["archived_at"].is_number())
                    .filter_map(|project| project["project_key"].as_str().map(str::to_owned)),
            )
            .chain(
                self.project_deletion_fences
                    .iter()
                    .map(|p| p.project_key.clone()),
            )
            .collect()
    }

    pub(in crate::app) fn refuse_archived_project(&self, id: &str) -> Result<(), ApiError> {
        self.refuse_deleting_address(id)
            .map_err(ApiError::classify)?;
        let name = self
            .projects
            .get(id)
            .filter(|p| p.archived_at.is_some())
            .map(|p| p.name.as_str())
            .or_else(|| {
                self.projects
                    .parked()
                    .iter()
                    .find(|p| p["id"] == id && p["archived_at"].is_number())
                    .and_then(|p| p["name"].as_str())
            });
        if let Some(name) = name {
            return Err(ApiError::ProjectArchived {
                message: format!("{name} is archived; restore it first"),
                details: Some(json!({"project_id":id})),
            });
        }
        Ok(())
    }

    pub(in crate::app) fn refuse_archived_entity(&self, owner: &str) -> Result<(), String> {
        self.refuse_deleting_address(owner)?;
        let id = self
            .projects
            .project_id_of(owner)
            .map(str::to_string)
            .or_else(|| self.project_id_for_archive_key(&self.projects.project_key_for(owner)));
        match id {
            Some(id) => self
                .refuse_archived_project(&id)
                .map_err(archive_error_string),
            None => Ok(()),
        }
    }

    pub(in crate::app) fn guard_archived_rpc(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<(), ApiError> {
        self.guard_deleting_rpc(method, params)?;
        if !archive_guard_required(method) {
            return Ok(());
        }
        if let Some(id) = params.get("project_id").and_then(Value::as_str) {
            self.refuse_archived_project(id)?;
        }
        if let Some(id) = params
            .get("workspace_id")
            .and_then(Value::as_str)
            .and_then(|id| self.workspaces.get(id))
            .map(|w| &w.project_id)
        {
            self.refuse_archived_project(id)?;
        }
        self.guard_archived_entity_params(params)?;
        if let Some(task) = params.get("task_id").and_then(Value::as_str) {
            self.guard_archived_task(task)?;
        }
        Ok(())
    }

    fn guard_archived_entity_params(&self, params: &Value) -> Result<(), ApiError> {
        for field in ["entity_id", "run_id", "plan_id", "id"] {
            if let Some(owner) = params.get(field).and_then(Value::as_str) {
                self.refuse_archived_entity(owner)
                    .map_err(ApiError::classify)?;
                self.refuse_run_recovery(owner)
                    .map_err(ApiError::classify)?;
            }
        }
        Ok(())
    }

    fn guard_archived_task(&self, task: &str) -> Result<(), ApiError> {
        let keys = self.task_admission_keys.borrow();
        if let Some(id) = keys
            .get(task)
            .and_then(|key| self.project_id_for_archive_key(key))
        {
            self.refuse_archived_project(&id)?;
        }
        Ok(())
    }

    pub(in crate::app) fn project_id_for_archive_key(&self, key: &str) -> Option<String> {
        self.project_by_key(key)
            .map(|p| p.id.clone())
            .or_else(|| {
                self.projects
                    .parked()
                    .iter()
                    .find(|p| p["project_key"].as_str() == Some(key))
                    .and_then(|p| p["id"].as_str())
                    .map(str::to_string)
            })
            .or_else(|| {
                self.project_deletion_fences
                    .iter()
                    .find(|p| p.project_key == key)
                    .map(|p| p.project_id.clone())
            })
    }

    pub(in crate::app) fn guard_archived_action(
        &self,
        owner: &str,
        action: &BridgeAction,
    ) -> Result<(), String> {
        self.refuse_deleting_address(owner)?;
        if !action_is_read(action) {
            self.refuse_archived_entity(owner)?;
            self.refuse_run_recovery(owner)?;
        }
        Ok(())
    }
}

pub(in crate::app) fn archive_error_string(error: ApiError) -> String {
    crate::api::v1::reviews::errors::encode(
        error.code(),
        error.message(),
        error.details().cloned().unwrap_or(Value::Null),
    )
}

pub(in crate::app) fn archive_guard_required(method: &str) -> bool {
    effects::requires_write_scope(method) == Some(true)
        && !matches!(
            method,
            "project.restore"
                | "project.delete"
                | "agent.attach"
                | "term.attach"
                | "term.ack"
                | "term.resize"
        )
}

fn action_is_read(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::ListHarnesses
            | BridgeAction::ListWorkspaces
            | BridgeAction::ListWorkspaceAgents { .. }
            | BridgeAction::ListProjectAgents
            | BridgeAction::ListProjects
            | BridgeAction::ListWork
            | BridgeAction::ReadConversation { .. }
            | BridgeAction::SearchConversation { .. }
            | BridgeAction::TrackerListTasks { .. }
            | BridgeAction::TrackerGetTask { .. }
            | BridgeAction::TrackerReadComment { .. }
            | BridgeAction::TrackerTaskDependents { .. }
            | BridgeAction::TrackerGetReview { .. }
            | BridgeAction::TrackerReadReview { .. }
    )
}
