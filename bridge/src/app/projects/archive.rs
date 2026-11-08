//! Archive retains identity and history; admission refuses rather than stopping work.
use crate::api::ApiError;
use crate::app::{AppState, TabKey};
use crate::project_roster::ProjectRole;
use crate::workspace::WorkspaceStatus;
use serde_json::{json, Value};

impl AppState {
    pub(crate) fn project_archive(&mut self, id: &str) -> Result<Value, ApiError> {
        self.refuse_archived_project(id)?;
        let project = self.project_for(id).map_err(ApiError::classify)?;
        let key = project.project_key.clone();
        let name = project.name.clone();
        let mut blockers = self.session_registry.retiring_project_blockers(id);
        blockers.extend(self.archive_workspace_blockers(id));
        blockers.extend(self.archive_session_blockers(id));
        if self.session_registry.pending_shell_for_project(id) {
            blockers.push("wait for terminal creation to finish".into());
        }
        if self.session_registry.live_shell_for_project(id) {
            blockers.push("close the open terminal in this project".into());
        }
        if let Some(store) = &self.store {
            blockers.extend(
                store
                    .project_archive_task_blockers(&key)
                    .map_err(|error| ApiError::unavailable(error.to_string()))?,
            );
        }
        blockers.extend(self.archive_creation_blockers(&key)?);
        if self.active_deferred_filesystem_jobs > 0 || self.deferred_work.is_some() {
            blockers.push("wait for filesystem operations to finish".into());
        }
        blockers.sort();
        if !blockers.is_empty() {
            return Err(ApiError::conflict(
                format!("Cannot archive {name}: {}", blockers.join("; ")),
                Some(json!({"project_id":id,"blockers":blockers})),
            ));
        }
        self.set_project_archive(id, Some(crate::agent::now_ms() as u64))
    }

    pub(crate) fn project_restore(&mut self, id: &str) -> Result<Value, ApiError> {
        let was_archived = self
            .project_for(id)
            .map_err(ApiError::classify)?
            .archived_at
            .is_some();
        let restored = self.set_project_archive(id, None)?;
        self.schedule_restored_run_recovery(Some(id));
        if was_archived {
            self.remember_restored_recipient_wakes(id);
        }
        self.replay_restored_provider_notices(id);
        self.release_restored_recipient_wakes();
        Ok(restored)
    }

    fn set_project_archive(&mut self, id: &str, at: Option<u64>) -> Result<Value, ApiError> {
        let project = self
            .commit_project_configuration(id, |projects| {
                projects.set_archived_at(id, at);
                Ok(())
            })
            .map_err(ApiError::classify)?;
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(json!({"project":project}))
    }

    fn archive_creation_blockers(&self, key: &str) -> Result<Vec<String>, ApiError> {
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let pending = store
            .writer()
            .pending_creations()
            .map_err(|error| ApiError::unavailable(error.to_string()))?;
        Ok(pending
            .iter()
            .filter(|record| record.execution["project_key"].as_str() == Some(key))
            .map(|record| {
                let name = record.execution["workspace"]["name"]
                    .as_str()
                    .unwrap_or(&record.operation_id);
                format!("wait for workspace {name} creation to finish")
            })
            .collect())
    }

    fn archive_workspace_blockers(&self, project: &str) -> Vec<String> {
        self.workspaces
            .list(Some(project))
            .into_iter()
            .filter(|workspace| workspace.status != WorkspaceStatus::Finished)
            .map(|workspace| format!("finish or delete workspace {}", workspace.name))
            .collect()
    }

    fn archive_session_blockers(&self, project: &str) -> Vec<String> {
        self.runs
            .keys()
            .chain(self.plans.keys())
            .filter(|owner| self.projects.project_id_of(owner) == Some(project))
            .flat_map(|owner| {
                let mut blockers = self.archive_owner_blockers(owner);
                if self.delivery_queue.archive_holds_owner(owner) {
                    blockers.push(format!("wait for pending wake for {owner}"));
                }
                blockers
            })
            .collect()
    }

    fn archive_owner_blockers(&self, owner: &str) -> Vec<String> {
        let (Ok(roster), Ok(root)) = (self.entity_agents(owner), self.entity_agent_root(owner))
        else {
            return Vec::new();
        };
        let root = Self::canonical_root(&root);
        roster
            .iter()
            .filter(|agent| self.agent_session_held(&TabKey::agent(&root, &agent.id)))
            .map(|agent| {
                let role = if agent.project_role == Some(ProjectRole::Coordinator) {
                    "coordinator"
                } else {
                    "worker"
                };
                format!("stop {role} {}", agent.name.as_deref().unwrap_or(&agent.id))
            })
            .collect()
    }
}
