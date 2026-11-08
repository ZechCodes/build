//! Environment rows consume producer-owned repository observations only.
use super::*;
use crate::app::environments::Environment;
use crate::app::projects::ProjectSource;
use crate::isolation::{Isolation, IsolationAvailability};

/// Immutable inputs captured by the maintenance producer before filesystem work.
#[derive(PartialEq, Eq)]
pub(super) struct EnvironmentSnapshotRow {
    id: String,
    sources: Vec<ProjectSource>,
    worktrees_root: PathBuf,
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
            let unchanged = current.contains(&row);
            let newer = observations
                .get(&row.id)
                .is_none_or(|old| old.version < version);
            if unchanged && newer {
                observations.insert(row.id, ProjectObservation { version, value });
            }
        }
        observations.retain(|id, _| current.iter().any(|row| row.id == *id));
    }

    /// Preserve the latest committed repository evidence before its last project goes.
    pub(in crate::app) fn retain_environment_snapshot_observation(&self, project: &str) {
        let Some(environment) = self.projects.get(project).map(|p| &p.environment_id) else {
            return;
        };
        let projects = self.snapshot_observations.projects.lock().unwrap();
        let Some(observation) = projects.get(project) else {
            return;
        };
        let mut environments = self.snapshot_observations.environments.lock().unwrap();
        if environments
            .get(environment)
            .is_none_or(|old| old.version < observation.version)
        {
            let home = self
                .projects
                .environment(environment)
                .and_then(|e| e.sources.first());
            let value =
                retained_environment_value(observation, environments.get(environment), home);
            environments.insert(
                environment.clone(),
                ProjectObservation {
                    version: observation.version,
                    value,
                },
            );
        }
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
                let remote = observed
                    .iter()
                    .copied()
                    .chain(cached)
                    .flat_map(|row| row.value["sources"].as_array().into_iter().flatten())
                    .find(|row| {
                        row["id"] == source.id && row["path"] == source.path.display().to_string()
                    })
                    .map(|row| row["remote"].clone())
                    .unwrap_or(Value::Null);
                source.wire_observed(remote)
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

fn retained_environment_value(
    project: &ProjectObservation,
    cached: Option<&ProjectObservation>,
    home: Option<&ProjectSource>,
) -> Value {
    let mut value = project.value.clone();
    if home.is_none_or(|source| value["path"] != source.path.display().to_string()) {
        value["path"] = cached
            .map(|old| old.value["path"].clone())
            .unwrap_or(Value::Null);
        value["isolation_available"] = cached
            .map(|old| old.value["isolation_available"].clone())
            .unwrap_or(Value::Null);
    }
    value
}

#[cfg(test)]
mod env_observation_tests;
