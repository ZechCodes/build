//! Stage project configuration and its read view before the durable commit.
use super::project_registry::ProjectConfiguration;
use super::ProjectRegistry;
use crate::app::AppState;
use serde_json::Value;

impl AppState {
    pub(in crate::app) fn commit_project_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        self.projects
            .get(id)
            .ok_or_else(|| format!("unknown project: {id}"))?;
        let mut configuration = ProjectConfiguration::capture(&self.projects, id);
        let previous = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut observation = None;
        self.swap_project_snapshot_observation(id, &mut observation);
        let changed = change(&mut self.projects).and_then(|()| {
            self.validate_secret_project(self.projects.get(id).expect("staged project"))
        });
        if let Err(error) = changed {
            configuration.swap(&mut self.projects, id);
            self.swap_project_snapshot_observation(id, &mut observation);
            return Err(error);
        }
        self.projects.reconcile_project_sources(id);
        let row = self.project_json(self.projects.get(id).expect("staged project"));
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let projection = self.prepare_all_snapshot_projection();
        configuration.swap(&mut self.projects, id);
        self.swap_project_snapshot_observation(id, &mut observation);
        // Git remotes live in Git, so an edit only observing a new remote has
        // no configuration to commit and must not fail an unrelated config IO.
        if config != previous {
            self.persist_config(&config)?;
        }
        configuration.swap(&mut self.projects, id);
        self.swap_project_snapshot_observation(id, &mut observation);
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        }
        Ok(row)
    }
}

impl AppState {
    /// Stage the owning references and every live projection, even with no projects.
    pub(in crate::app) fn commit_environment_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        self.projects
            .environment(id)
            .ok_or_else(|| format!("unknown environment: {id}"))?;
        let mut configuration = ProjectConfiguration::capture_environment(&self.projects, id);
        let previous = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        if let Err(error) = change(&mut self.projects) {
            configuration.swap(&mut self.projects, id);
            return Err(error);
        }
        self.projects.refresh_environment_projects(id);
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut observation = self.prepare_environment_edit_observation(id);
        self.swap_environment_snapshot_observation(id, &mut observation);
        let row = self
            .environment_json(id)
            .expect("edited environment remains registered");
        let projection = self.prepare_all_snapshot_projection();
        configuration.swap(&mut self.projects, id);
        self.swap_environment_snapshot_observation(id, &mut observation);
        if config != previous {
            self.persist_config(&config)?;
        }
        configuration.swap(&mut self.projects, id);
        self.swap_environment_snapshot_observation(id, &mut observation);
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        }
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(row)
    }
}
