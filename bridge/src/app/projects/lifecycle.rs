use super::ProjectSource;
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
    pub(in crate::app) fn register_opened_project_sources(
        &mut self,
        opened: crate::lifecycle::OpenedRepository,
        sources: Vec<ProjectSource>,
        created_checkouts: Vec<std::path::PathBuf>,
    ) -> Result<Value, String> {
        if let Some(existing_name) = self
            .projects
            .find_by_canonical_path(&opened.path)
            .map(|existing| existing.name.clone())
        {
            for path in created_checkouts {
                self.run_off_lock(RemoveUnregisteredProject { path });
            }
            return Err(format!("project already registered: {existing_name}"));
        }
        let project =
            self.project_candidate_with_sources(opened.path, opened.base, opened.is_git, sources);
        let config = self.config_value_with_project(
            &self.projects_dir,
            self.default_harness,
            self.isolation,
            Some(project.project()),
        );
        if let Err(error) = self.persist_config(&config) {
            for path in created_checkouts {
                self.run_off_lock(RemoveUnregisteredProject { path });
            }
            return Err(error);
        }
        let reply = self.project_json(project.project());
        self.insert_project(project);
        Ok(reply)
    }

    /// Append the sources one `project.add_source` opened.
    ///
    /// The project may have gone while the clone ran; a checkout this call
    /// made has nowhere to belong then, so it is walked away the same way a
    /// failed registration's is.
    pub(in crate::app) fn append_project_sources(
        &mut self,
        project_id: &str,
        added: Vec<ProjectSource>,
        created_checkouts: Vec<std::path::PathBuf>,
    ) -> Result<Value, String> {
        let Some(project) = self.projects.get(project_id) else {
            for path in created_checkouts {
                self.run_off_lock(RemoveUnregisteredProject { path });
            }
            return Err(format!("unknown project: {project_id}"));
        };
        let mut sources = project.sources.clone();
        sources.extend(added);
        assert!(
            self.projects.set_sources(project_id, sources),
            "the project was just resolved"
        );
        self.persist();
        let project = self
            .projects
            .get(project_id)
            .expect("the project was just resolved");
        Ok(self.project_json(project))
    }

    /// Register the repository opened by any project-creation door.
    pub(in crate::app) fn register_opened_repository(
        &mut self,
        opened: crate::lifecycle::OpenedRepository,
    ) -> Result<Value, String> {
        if let Some(existing) = self.projects.find_by_canonical_path(&opened.path) {
            return Ok(self.project_json(existing));
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
        let reply = self.project_json(project.project());
        self.insert_project(project);
        Ok(reply)
    }

    pub(in crate::app) fn settle_remote_change(&self, project_id: &str) -> Result<Value, String> {
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        Ok(self.project_json(project))
    }
}
