use super::*;
use crate::app::require_str;
use serde_json::{json, Value};

impl AppState {
    pub(in crate::app) fn create_environment_project(
        &mut self,
        environment_id: &str,
        name: &str,
        key: &str,
        provisional_id: &str,
        added: Vec<ProjectSource>,
        checkpoint_identity: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<String, String> {
        if let Some(project) = self
            .projects
            .iter()
            .find(|project| project.project_key == key)
        {
            if project.environment_id != environment_id {
                return Err("saved creation identity no longer matches the project".into());
            }
            checkpoint_identity(&project.id)?;
            return Ok(project.id.clone());
        }
        if let Some(project) = self
            .projects
            .parked()
            .iter()
            .find(|p| p["project_key"] == key)
        {
            if project["environment_id"] != environment_id {
                return Err("saved creation identity no longer matches the parked project".into());
            }
            let id = project["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or("saved creation project id missing")?;
            checkpoint_identity(&id)?;
            return Ok(id);
        }
        let home = self
            .projects
            .environment(environment_id)
            .and_then(|e| e.sources.first())
            .ok_or("environment has no home source")?
            .clone();
        // The key proves no project landed. A legacy allocator may have passed
        // the checkpoint's numeric ID, including while the bridge was stopped.
        // Choose and publish against its current floor under the same app lock.
        let number = provisional_id
            .strip_prefix("proj-")
            .and_then(|n| n.parse::<u64>().ok())
            .ok_or("invalid reserved project id")?
            .max(self.projects.next_id());
        let candidate = self.project_candidate_with_sources_at(
            home.path,
            home.base_branch,
            home.is_git,
            Vec::new(),
            Some(number),
        );
        let mut candidate =
            self.projects
                .add_project_to_environment(environment_id, name, key, candidate)?;
        let ids = self
            .projects
            .allocate_environment_source_ids(environment_id, added.len())?;
        let added = added
            .into_iter()
            .zip(ids)
            .map(|(mut source, id)| {
                source.id = id;
                source
            })
            .collect();
        candidate.add_creation_sources(added);
        let config = self.config_value_with_project(
            &self.projects_dir,
            self.default_harness,
            self.isolation,
            Some(candidate.project()),
        );
        // Keep selection, write-ahead identity and config publication under the
        // same app lock. No allocator can take this ID after it is checkpointed.
        // The callback is SQL-only and must succeed before any config write.
        checkpoint_identity(&candidate.project().id)?;
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
        if !matches!(
            record.method.as_str(),
            "environment.create" | "environment.create_project"
        ) || record.execution["project_id"] != id
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

impl AppState {
    pub(in crate::app) fn validate_creation_environment_sources(
        &self,
        sources: &mut [ProjectSource],
        cloned: &[std::path::PathBuf],
    ) -> Result<(), String> {
        self.validate_prepared_sources(None, sources, cloned)
    }
    pub(in crate::app) fn persist_creation_environment(
        &mut self,
        environment: crate::app::environments::Environment,
    ) -> Result<(), String> {
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["environments"].as_array_mut().unwrap().push(serde_json::json!({
            "id":environment.id,"name":environment.name,"next_source":environment.next_source,
            "isolation":environment.isolation,"sources":environment.sources.iter().map(ProjectSource::wire).collect::<Vec<_>>()
        }));
        self.persist_config(&config)?;
        self.projects.publish_creation_environment(environment);
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(())
    }
    pub(in crate::app) fn validate_creation_additions(
        &self,
        environment_id: &str,
        added: &mut [ProjectSource],
        cloned: &[std::path::PathBuf],
    ) -> Result<(), String> {
        let inherited = self
            .projects
            .environment(environment_id)
            .ok_or("unknown environment")?
            .sources
            .clone();
        for source in added.iter() {
            let path=crate::worktree::canonical_planned_path(&source.path);
            if inherited.iter().any(|s|crate::worktree::canonical_planned_path(&s.path)==path) {
                return Err(format!("source is already included in this project: {}",path.display()));
            }
        }
        // Existing environment references can be temporarily unavailable. New
        // additions are checked without changing M0.4's landed-project behavior.
        self.validate_prepared_sources(None, added, cloned)
    }
}

impl AppState {
    pub(in crate::app) fn check_project_source_edit(
        &self,
        project_id: &str,
        source_id: &str,
        legacy: bool,
    ) -> Result<(), String> {
        let project = self.project_for(project_id)?;
        let inherited = self
            .projects
            .environment(&project.environment_id)
            .is_some_and(|e| e.sources.iter().any(|s| s.id == source_id));
        if inherited && !(legacy && project.added_sources.is_empty()) {
            return Err(format!(
                "{source_id} belongs to environment {}; edit it there",
                project.environment_id
            ));
        }
        Ok(())
    }
}
