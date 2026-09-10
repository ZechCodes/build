use crate::app::{AppState, OffLockJob};
use crate::lifecycle::LifecycleEpilogue;
use serde_json::Value;

/// Every project door's apply half: a repository is on disk, read, and ready to
/// be registered. `project.add`, `project.clone` and `project.create` differ
/// only in how the directory got there.
pub struct ProjectAdded {
    pub path: std::path::PathBuf,
    pub base: String,
    pub remote: Option<String>,
    pub created_checkout: Option<std::path::PathBuf>,
}

impl LifecycleEpilogue for ProjectAdded {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        // Idempotent, as it has always been: a repository already registered
        // under this canonical path answers with the project it already is.
        if let Some(existing) = state
            .projects
            .iter()
            .find(|project| project.repo_path == self.path)
        {
            return Ok(state.project_json(existing, self.remote));
        }
        let project = state.project_candidate(self.path, self.base);
        let config = state.config_value_with_project(
            &state.projects_dir,
            state.default_harness,
            state.isolation,
            Some(&project),
        );
        if let Err(error) = state.persist_config(&config) {
            if let Some(path) = self.created_checkout {
                state.run_off_lock(RemoveUnregisteredProject { path });
            }
            return Err(error);
        }
        let reply = state.project_json(&project, self.remote);
        state.insert_project(project);
        Ok(reply)
    }
}

/// A failed registration owns only the checkout its own run phase created.
struct RemoveUnregisteredProject {
    path: std::path::PathBuf,
}

impl OffLockJob for RemoveUnregisteredProject {
    type Claim = ();
    type Decided = ();
    fn claim(&self) {}
    fn decide(self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!(
                "cannot remove unregistered project {}: {error}",
                self.path.display()
            );
        }
    }
    fn apply(_state: &mut AppState, (): (), (): ()) {}
    fn abandon(_state: &mut AppState, (): ()) {}
}

/// `project.set_remote`'s apply half: Git owns the remote configuration; the
/// app's persisted project configuration has not changed.
pub struct ProjectRemoteSet {
    pub project_id: String,
    pub remote: Option<String>,
}

impl LifecycleEpilogue for ProjectRemoteSet {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let project = state
            .projects
            .iter()
            .find(|project| project.id == self.project_id)
            .ok_or_else(|| format!("unknown project: {}", self.project_id))?;
        Ok(state.project_json(project, self.remote))
    }
}
