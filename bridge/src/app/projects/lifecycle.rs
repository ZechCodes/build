use crate::app::{AppState, OffLockJob};
use serde_json::Value;

/// A failed registration owns only the checkout its own run phase created.
struct RemoveUnregisteredProject {
    path: std::path::PathBuf,
}

impl OffLockJob for RemoveUnregisteredProject {
    type Claim = ();
    type Decided = ();
    fn claim(&mut self) {}
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

impl AppState {
    /// Register the repository opened by any project-creation door.
    pub(in crate::app) fn register_opened_repository(
        &mut self,
        opened: crate::lifecycle::OpenedRepository,
    ) -> Result<Value, String> {
        if let Some(existing) = self.projects.find_by_canonical_path(&opened.path) {
            return Ok(self.project_json(existing, opened.remote));
        }
        let project = self.project_candidate(opened.path, opened.base, opened.is_git);
        let config = self.config_value_with_project(
            &self.projects_dir,
            self.default_harness,
            self.isolation,
            Some(project.project()),
        );
        if let Err(error) = self.persist_config(&config) {
            if let Some(path) = opened.created_checkout {
                self.run_off_lock(RemoveUnregisteredProject { path });
            }
            return Err(error);
        }
        let reply = self.project_json(project.project(), opened.remote);
        self.insert_project(project);
        Ok(reply)
    }

    pub(in crate::app) fn settle_remote_change(
        &self,
        project_id: &str,
        changed: crate::lifecycle::RemoteChanged,
    ) -> Result<Value, String> {
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        Ok(self.project_json(project, changed.remote))
    }
}
