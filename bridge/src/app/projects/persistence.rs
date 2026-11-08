//! Stage project configuration and its read view before the durable commit.
use super::project_registry::ProjectConfiguration;
use super::ProjectRegistry;
use crate::app::snapshot_projection::{ProjectSnapshotRefresh, RenderedProjectSnapshotRefresh};
use crate::app::AppState;
use serde_json::Value;

impl AppState {
    pub(in crate::app) fn commit_project_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<Value, String> {
        self.commit_project_configuration_with_observation(id, change, None)
    }

    pub(in crate::app) fn capture_prospective_project_refresh(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<ProjectSnapshotRefresh, String> {
        let mut configuration = ProjectConfiguration::capture(&self.projects, id);
        let result = change(&mut self.projects).map(|()| self.capture_project_snapshot_refresh());
        configuration.swap(&mut self.projects, id);
        result
    }

    pub(in crate::app) fn commit_project_configuration_with_observation(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
        observed: Option<RenderedProjectSnapshotRefresh>,
    ) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        self.projects
            .get(id)
            .ok_or_else(|| format!("unknown project: {id}"))?;
        let mut configuration = ProjectConfiguration::capture(&self.projects, id);
        let previous = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut observation = self.capture_configuration_observations();
        let changed = change(&mut self.projects).and_then(|()| {
            self.validate_secret_project(self.projects.get(id).expect("staged project"))
        });
        if let Err(error) = changed {
            configuration.swap(&mut self.projects, id);
            self.swap_configuration_observations(&mut observation);
            return Err(error);
        }
        self.projects.reconcile_project_sources(id);
        let row = if let Some(observed) = &observed {
            self.install_project_snapshot_refresh(observed.clone());
            self.project_json_observed(id)?
        } else {
            self.project_json(self.projects.get(id).expect("staged project"))
        };
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let projection = self.prepare_all_snapshot_projection();
        configuration.swap(&mut self.projects, id);
        self.swap_configuration_observations(&mut observation);
        // Git remotes live in Git, so an edit only observing a new remote has
        // no configuration to commit and must not fail an unrelated config IO.
        if config != previous {
            if let Err(error) = self.persist_config(&config) {
                if let Some(observed) = observed {
                    self.install_project_snapshot_refresh(observed);
                }
                return Err(error);
            }
        }
        configuration.swap(&mut self.projects, id);
        self.swap_configuration_observations(&mut observation);
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        }
        Ok(row)
    }
}
