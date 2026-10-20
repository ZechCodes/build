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
        let project = self
            .projects
            .get(id)
            .ok_or_else(|| format!("unknown project: {id}"))?;
        let mut configuration = ProjectConfiguration::capture(project);
        let previous = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut observation = None;
        self.swap_project_snapshot_observation(id, &mut observation);
        if let Err(error) = change(&mut self.projects) {
            configuration.swap(&mut self.projects, id);
            self.swap_project_snapshot_observation(id, &mut observation);
            return Err(error);
        }
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
