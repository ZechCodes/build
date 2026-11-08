//! Cached project repository and volume observations.
use super::*;

impl AppState {
    pub(in crate::app) fn capture_project_snapshot_observations(&self) -> Vec<ProjectListRow> {
        self.project_list_rows()
    }
    /// Call after rendering the captured rows off the app lock. The caller
    /// keeps the lifecycle generation fence through this installation.
    pub(in crate::app) fn install_project_snapshot_observations(
        &mut self,
        rows: Vec<(String, Value)>,
    ) {
        let version = self.read_view.next_version();
        let current = self.project_list_rows();
        let mut observations = self.snapshot_observations.projects.lock().unwrap();
        for (id, value) in rows {
            if let Some(row) = current.iter().find(|row| row.project_id == id) {
                publish_source_observation(
                    &mut observations,
                    &id,
                    ProjectObservation::new(version, value),
                    &row.sources,
                    &row.repo_path,
                    true,
                );
            }
        }
        observations.retain(|id, _| current.iter().any(|row| row.project_id == *id));
    }

    pub(in crate::app) fn capture_project_snapshot_refresh(&self) -> ProjectSnapshotRefresh {
        ProjectSnapshotRefresh {
            version: self.read_view.next_version(),
            rows: self.project_list_rows(),
            environments: self.capture_environment_snapshot_rows(),
            claims: vec![],
        }
    }
    pub(in crate::app) fn install_project_snapshot_refresh(
        &self,
        refresh: RenderedProjectSnapshotRefresh,
    ) {
        self.install_environment_snapshot_refresh(refresh.version, refresh.environments);
        let current = self.project_list_rows();
        let mut observations = self.snapshot_observations.projects.lock().unwrap();
        for (row, value) in refresh.rows {
            if let Some(current) = current
                .iter()
                .find(|current| current.project_id == row.project_id)
            {
                publish_source_observation(
                    &mut observations,
                    &row.project_id,
                    ProjectObservation::new(refresh.version, value),
                    &current.sources,
                    &current.repo_path,
                    current.worktrees_root == row.worktrees_root,
                );
            }
        }
        observations.retain(|id, _| current.iter().any(|row| row.project_id == *id));
    }
    pub(in crate::app) fn has_current_project_snapshot_observation(
        &self,
        project: &crate::app::projects::Project,
    ) -> bool {
        let observations = self.snapshot_observations.projects.lock().unwrap();
        let Some(observed) = observations.get(&project.id) else {
            return false;
        };
        let Some(sources) = observed.value["sources"].as_array() else {
            return false;
        };
        observed.value["path"] == project.repo_path.display().to_string()
            && sources.len() == project.sources.len()
            && project.sources.iter().all(|source| {
                sources.iter().any(|row| {
                    row["id"] == source.id
                        && row["path"] == source.path.display().to_string()
                        && row["is_git"] == source.is_git
                })
            })
    }
    pub(in crate::app) fn record_project_snapshot_observation(&self, project: &str, value: &Value) {
        if let Some(current) = self.projects.get(project) {
            publish_source_observation(
                &mut self.snapshot_observations.projects.lock().unwrap(),
                project,
                ProjectObservation::new(self.read_view.next_version(), value.clone()),
                &current.sources,
                &current.home_path,
                true,
            );
        }
    }

    pub(super) fn snapshot_account_availability(&self) -> Value {
        self.projects
            .iter()
            .next()
            .and_then(|project| {
                self.snapshot_observations
                    .projects
                    .lock()
                    .unwrap()
                    .get(&project.id)
                    .map(|observed| observed.value["isolation_available"].clone())
            })
            .unwrap_or_else(|| {
                json!(crate::isolation::IsolationAvailability::unavailable(
                    "no project registered yet"
                ))
            })
    }
    pub(super) fn snapshot_project_list(&self) -> Value {
        let observed = self.snapshot_observations.projects.lock().unwrap();
        let projects = self
            .project_list_rows()
            .iter()
            .map(|row| {
                let observation = observed
                    .get(&row.project_id)
                    .expect("live project observations warmed before publication");
                row.render_observed(
                    observation.value["isolation_available"].clone(),
                    Some(&observation.value),
                )
            })
            .collect::<Vec<_>>();
        json!({"projects": projects})
    }
}
