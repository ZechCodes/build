//! Environment rows consume producer-owned repository observations only.
use super::*;
use crate::app::environments::Environment;
use crate::app::projects::ProjectSource;
use crate::isolation::{Isolation, IsolationAvailability};

/// Immutable inputs captured by the maintenance producer before filesystem work.
#[derive(Clone, Eq)]
pub(super) struct EnvironmentSnapshotRow {
    pub(super) id: String,
    pub(super) sources: Vec<ProjectSource>,
    worktrees_root: PathBuf,
}

impl PartialEq for EnvironmentSnapshotRow {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.worktrees_root == other.worktrees_root
            && super::source_observation_inputs_match(&self.sources, &other.sources)
    }
}

impl EnvironmentSnapshotRow {
    pub(super) fn render(&self) -> Value {
        let available = self
            .sources
            .first()
            .map(|source| IsolationAvailability::of(&source.path, &self.worktrees_root))
            .unwrap_or_else(|| IsolationAvailability::unavailable("environment has no source"));
        json!({
            "path":self.sources.first().map(|source|source.path.display().to_string()),
            "sources":self.sources.iter().map(ProjectSource::wire).collect::<Vec<_>>(),
            "isolation_available":available,
        })
    }
}

impl AppState {
    pub(super) fn capture_environment_snapshot_rows(&self) -> Vec<EnvironmentSnapshotRow> {
        self.projects
            .environments()
            .map(|environment| EnvironmentSnapshotRow {
                id: environment.id.clone(),
                sources: environment.sources.clone(),
                worktrees_root: self.worktrees_root.clone(),
            })
            .collect()
    }

    /// Startup is unshared; later probes render off-lock with project refreshes.
    pub(super) fn warm_environment_snapshot_observations(&self) {
        let rows = self
            .capture_environment_snapshot_rows()
            .into_iter()
            .map(|row| {
                let value = row.render();
                (row, value)
            })
            .collect();
        self.install_environment_snapshot_refresh(self.read_view.next_version(), rows);
    }

    pub(super) fn install_environment_snapshot_refresh(
        &self,
        version: u64,
        rows: Vec<(EnvironmentSnapshotRow, Value)>,
    ) {
        let current = self.capture_environment_snapshot_rows();
        let mut observations = self.snapshot_observations.environments.lock().unwrap();
        for (row, value) in rows {
            if let Some(current) = current.iter().find(|current| current.id == row.id) {
                let home = current
                    .sources
                    .first()
                    .map(|s| s.path.as_path())
                    .unwrap_or(std::path::Path::new(""));
                publish_source_observation(
                    &mut observations,
                    &row.id,
                    ProjectObservation::new(version, value),
                    &current.sources,
                    home,
                    current.worktrees_root == row.worktrees_root,
                );
            }
        }
        observations.retain(|id, _| current.iter().any(|row| row.id == *id));
    }

    /// Preserve the latest committed repository evidence before its last project goes.
    pub(in crate::app) fn retain_environment_snapshot_observation(&self, project: &str) {
        let Some(environment) = self.projects.get(project).map(|p| &p.environment_id) else {
            return;
        };
        let observation = self
            .snapshot_observations
            .projects
            .lock()
            .unwrap()
            .get(project)
            .cloned();
        let (Some(observation), Some(current)) =
            (observation, self.projects.environment(environment))
        else {
            return;
        };
        let home = current
            .sources
            .first()
            .map(|s| s.path.as_path())
            .unwrap_or(std::path::Path::new(""));
        publish_source_observation(
            &mut self.snapshot_observations.environments.lock().unwrap(),
            environment,
            observation,
            &current.sources,
            home,
            true,
        );
    }

    pub(crate) fn environment_list(&self) -> Value {
        self.snapshot_environment_list()
    }

    pub(in crate::app) fn snapshot_environment_list(&self) -> Value {
        let observations = self.snapshot_observations.projects.lock().unwrap();
        let environments = self.snapshot_observations.environments.lock().unwrap();
        let rows = self
            .projects
            .environments()
            .map(|environment| {
                self.environment_row(
                    environment,
                    &observations,
                    environments.get(&environment.id),
                )
            })
            .collect::<Vec<_>>();
        json!({"environments": rows})
    }

    fn environment_row(
        &self,
        environment: &Environment,
        observations: &BTreeMap<String, ProjectObservation>,
        cached: Option<&ProjectObservation>,
    ) -> Value {
        let projects = self
            .projects
            .iter()
            .filter(|p| p.environment_id == environment.id)
            .collect::<Vec<_>>();
        let observed = projects
            .iter()
            .filter_map(|p| observations.get(&p.id))
            .collect::<Vec<_>>();
        let sources = environment
            .sources
            .iter()
            .map(|source| {
                source.wire_observed(environment_source_remote(source, &observed, cached))
            })
            .collect::<Vec<_>>();
        let home = environment.sources.first();
        let requested = environment.isolation.unwrap_or(self.isolation);
        let rift_available = environment_rift_available(home, &observed, cached);
        let effective = if requested == Isolation::Rift && !rift_available {
            Isolation::default()
        } else {
            requested
        };
        json!({
            "environment_id":environment.id, "name":environment.name,
            "home_path":home.map(|s|s.path.display().to_string()).unwrap_or_default(),
            "is_git":home.is_some_and(|s|s.is_git),
            "base_branch":home.map(|s|s.base_branch.as_str()).unwrap_or_default(),
            "sources":sources, "isolation":environment.isolation, "isolation_effective":effective,
            "project_ids":projects.iter().map(|p|p.id.as_str()).collect::<Vec<_>>(), "device":self.device_name,
        })
    }

    pub(in crate::app) fn refresh_project_task_counts(&self, project: &str) -> Result<(), String> {
        let counts = match &self.store {
            Some(store) => store
                .tracker_project_counts(&self.tracker_project_path(project)?)
                .map_err(|e| e.to_string())?,
            None => (0, 0),
        };
        self.snapshot_observations
            .project_task_counts
            .lock()
            .unwrap()
            .insert(project.to_owned(), counts);
        Ok(())
    }

    pub(in crate::app) fn project_task_counts(&self, project: &str) -> (u64, u64) {
        self.snapshot_observations
            .project_task_counts
            .lock()
            .unwrap()
            .get(project)
            .copied()
            .unwrap_or_default()
    }
}

fn environment_source_remote(
    source: &ProjectSource,
    projects: &[&ProjectObservation],
    cached: Option<&ProjectObservation>,
) -> Value {
    let path = source.path.display().to_string();
    projects
        .iter()
        .copied()
        .chain(cached)
        .filter_map(|observed| {
            let sources = observed.value["sources"].as_array()?;
            let row = sources
                .iter()
                .find(|row| row["id"] == source.id && row["path"] == path)?;
            Some((observed.source_version(row), &row["remote"]))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, remote)| remote.clone())
        .unwrap_or(Value::Null)
}

/// A project's legacy home can differ from the environment's first source.
fn environment_rift_available(
    home: Option<&ProjectSource>,
    projects: &[&ProjectObservation],
    cached: Option<&ProjectObservation>,
) -> bool {
    let Some(home) = home else {
        return false;
    };
    let path = home.path.display().to_string();
    projects
        .iter()
        .copied()
        .chain(cached)
        .filter(|observed| observed.value["path"] == path)
        .max_by_key(|observed| observed.version)
        .is_some_and(|observed| observed.value["isolation_available"]["rift"] == true)
}

#[cfg(test)]
mod env_observation_tests;
