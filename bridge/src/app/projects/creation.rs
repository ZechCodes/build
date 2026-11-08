use super::*;
use crate::app::require_str;
use serde_json::{json, Value};

impl AppState {
    pub(in crate::app) fn create_environment_project(
        &mut self,
        environment_id: &str,
        name: &str,
        key: &str,
        expected_id: &str,
    ) -> Result<String, String> {
        if let Some(project) = self
            .projects
            .iter()
            .find(|project| project.project_key == key)
        {
            if project.id != expected_id || project.environment_id != environment_id {
                return Err("saved creation identity no longer matches the project".into());
            }
            return Ok(project.id.clone());
        }
        if let Some(project) = self
            .projects
            .parked()
            .iter()
            .find(|p| p["project_key"] == key)
        {
            if project["id"] != expected_id || project["environment_id"] != environment_id {
                return Err("saved creation identity no longer matches the parked project".into());
            }
            return Ok(expected_id.into());
        }
        let home = self
            .projects
            .environment(environment_id)
            .and_then(|e| e.sources.first())
            .ok_or("environment has no home source")?
            .clone();
        let number = expected_id
            .strip_prefix("proj-")
            .and_then(|n| n.parse().ok())
            .ok_or("invalid reserved project id")?;
        let candidate = self.project_candidate_with_sources_at(
            home.path,
            home.base_branch,
            home.is_git,
            Vec::new(),
            Some(number),
        );
        let candidate =
            self.projects
                .add_project_to_environment(environment_id, name, key, candidate)?;
        let config = self.config_value_with_project(
            &self.projects_dir,
            self.default_harness,
            self.isolation,
            Some(candidate.project()),
        );
        self.persist_config(&config)?;
        Ok(self.insert_project(candidate))
    }

    pub(crate) fn project_discard_unused(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "project_id")?;
        let operation = require_str(params, "creation_operation_id")?;
        let store = self.store.as_ref().ok_or("discard needs a durable store")?;
        let record = store
            .writer()
            .creation_record(super::super::creation::PRINCIPAL, &operation)
            .map_err(|e| e.to_string())?
            .ok_or("creation attempt not found")?;
        let project = self.project_for(&id)?;
        if record.method != "environment.create_project"
            || record.execution["project_id"] != id
            || record.execution["project_key"] != project.project_key
        {
            return Err("project was not created by this attempt".into());
        }
        if !self.workspaces.list(Some(&id)).is_empty() {
            return Err("project has a workspace".into());
        }
        if !matches!(record.state.as_str(), "succeeded" | "failed") {
            return Err("creation is still checking or needs cleanup".into());
        }
        self.refuse_pending_creation(&id)?;
        self.refuse_used_creation_project(&id, &project.project_key)?;
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["projects"]
            .as_array_mut()
            .expect("projects array")
            .retain(|p| p["id"] != id);
        let mut policies = self.compaction_policies.clone();
        policies.forget_project(&id);
        policies.write_config(&mut config);
        store
            .writer()
            .secrets_retire_project(&id)
            .map_err(|e| e.to_string())?;
        self.persist_config(&config)?;
        self.compaction_policies = policies;
        self.projects.remove(&id);
        self.board.diff_mut().remove_project(&id);
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(json!({}))
    }

    fn refuse_used_creation_project(&self, id: &str, key: &str) -> Result<(), String> {
        if self
            .plans
            .keys()
            .chain(self.runs.keys())
            .any(|owner| self.projects.project_id_of(owner) == Some(id))
        {
            return Err("project has a task or agent session".into());
        }
        if self
            .session_registry
            .projection_agent_owners()
            .iter()
            .any(|owner| owner == id || self.projects.project_id_of(owner) == Some(id))
            || self.session_registry.pending_shell_for_project(id)
        {
            return Err("project has an agent session".into());
        }
        let store = self.store.as_ref().ok_or("discard needs a durable store")?;
        let routed = self.tracker_project_path(id)?;
        if !store
            .list_tracker_tasks(key, crate::store::TaskFilter::default())
            .map_err(|e| e.to_string())?
            .is_empty()
        {
            return Err("project has a task".into());
        }
        if routed != key
            && !store
                .list_tracker_tasks(&routed, crate::store::TaskFilter::default())
                .map_err(|e| e.to_string())?
                .is_empty()
        {
            return Err("project has a task under its current routing key".into());
        }
        if store
            .load_all_run_rosters()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|r| r.project_path == key || r.project_path == routed)
            || store
                .load_all_plan_rosters()
                .map_err(|e| e.to_string())?
                .iter()
                .any(|r| r.project_path == key || r.project_path == routed)
        {
            return Err("project has a retained agent session".into());
        }
        Ok(())
    }
}
