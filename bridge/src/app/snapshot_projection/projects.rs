//! Cached project repository and volume observations.
use super::*;

impl AppState {
    pub(in crate::app) fn swap_project_snapshot_observation(
        &self,
        id: &str,
        candidate: &mut Option<ProjectObservation>,
    ) {
        let mut observations = self.snapshot_observations.projects.lock().unwrap();
        let previous = observations.remove(id);
        if let Some(candidate) = candidate.take() {
            observations.insert(id.to_owned(), candidate);
        }
        *candidate = previous;
    }

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
        *self.snapshot_observations.projects.lock().unwrap() = rows
            .into_iter()
            .map(|(id, value)| (id, ProjectObservation { version, value }))
            .collect();
    }
    pub(in crate::app) fn capture_project_snapshot_refresh(&self) -> ProjectSnapshotRefresh {
        ProjectSnapshotRefresh {
            version: self.read_view.next_version(),
            rows: self.project_list_rows(),
        }
    }
    pub(in crate::app) fn install_project_snapshot_refresh(
        &self,
        refresh: RenderedProjectSnapshotRefresh,
    ) {
        let current = self.project_list_rows();
        let mut observations = self.snapshot_observations.projects.lock().unwrap();
        for (row, value) in refresh.rows {
            let unchanged = current.iter().any(|current| {
                current.project_id == row.project_id
                    && current.repo_path == row.repo_path
                    && current.worktrees_root == row.worktrees_root
                    && current.sources == row.sources
            });
            let newer = observations
                .get(&row.project_id)
                .is_none_or(|old| old.version < refresh.version);
            if unchanged && newer {
                observations.insert(
                    row.project_id,
                    ProjectObservation {
                        version: refresh.version,
                        value,
                    },
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
        self.snapshot_observations.projects.lock().unwrap().insert(
            project.to_owned(),
            ProjectObservation {
                version: self.read_view.next_version(),
                value: value.clone(),
            },
        );
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
