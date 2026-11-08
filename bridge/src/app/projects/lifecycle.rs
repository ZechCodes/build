use super::source_directory_claims::{paths_overlap, SourceDirectoryLease};
use super::{canonical_source_path, ProjectSource};
use crate::app::{AppState, OffLockJob};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A failed registration owns only the unchanged checkout nobody has referenced.
struct RemoveUnregisteredProject {
    path: PathBuf,
    lease: SourceDirectoryLease,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

impl OffLockJob for RemoveUnregisteredProject {
    type Claim = SourceDirectoryLease;
    type Decided = ();
    fn claim(&mut self) -> Self::Claim {
        self.lease.clone()
    }
    fn decide(self) {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        if !self.lease.matches_directory() {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!(
                "cannot remove unregistered project {}: {error}",
                self.path.display()
            );
        }
    }
    fn apply(_state: &mut AppState, _claim: Self::Claim, (): ()) {}
    fn abandon(_state: &mut AppState, _claim: Self::Claim) {}
}

impl AppState {
    fn source_directory_has_owner(&self, path: &Path) -> bool {
        let project_owner = self
            .projects
            .registered_sources()
            .iter()
            .any(|source| paths_overlap(path, &source.path));
        let home_owner = self
            .projects
            .iter()
            .flat_map(|project| [&project.home_path, &project.repo_path])
            .any(|home| paths_overlap(path, &crate::worktree::canonical_planned_path(home)));
        let parked_home_owner = self
            .projects
            .parked()
            .iter()
            .flat_map(|project| ["home_path", "path"].map(|field| project.get(field)))
            .flatten()
            .filter_map(Value::as_str)
            .filter(|home| !home.is_empty())
            .any(|home| {
                paths_overlap(
                    path,
                    &crate::worktree::canonical_planned_path(&crate::app::expand_tilde(home)),
                )
            });
        project_owner
            || home_owner
            || parked_home_owner
            || self
                .workspaces
                .list(None)
                .into_iter()
                .flat_map(|workspace| {
                    std::iter::once(&workspace.root).chain(
                        workspace
                            .directories
                            .iter()
                            .flat_map(|directory| [&directory.source_path, &directory.path]),
                    )
                })
                .filter(|path| !path.as_os_str().is_empty())
                .any(|root| paths_overlap(path, &crate::worktree::canonical_planned_path(root)))
    }

    fn cleanup_unregistered_sources(&mut self, created: Vec<PathBuf>) {
        for path in created {
            if self.source_directory_has_owner(&path) {
                continue;
            }
            if let Some(lease) = self.source_directory_claims.begin_cleanup(&path) {
                self.run_off_lock(RemoveUnregisteredProject {
                    path: lease.path().to_path_buf(),
                    lease,
                    #[cfg(test)]
                    gate: self.off_lock_gate.clone(),
                });
            }
        }
    }

    pub(in crate::app) fn register_opened_project_sources(
        &mut self,
        mut opened: crate::lifecycle::OpenedRepository,
        mut sources: Vec<ProjectSource>,
        created_checkouts: Vec<PathBuf>,
        cloned_paths: Vec<PathBuf>,
    ) -> Result<Value, String> {
        let value = (|| {
            opened.path = canonical_source_path(&opened.path)?;
            self.validate_secret_source(&opened.path)?;
            self.refuse_source_directory_claim(&opened.path)?;
            self.validate_prepared_sources(None, &mut sources, &cloned_paths)?;
            if let Some(existing) = self.projects.find_by_canonical_path(&opened.path) {
                return Err(format!("project already registered: {}", existing.name));
            }
            let project = self.project_candidate_with_sources(
                opened.path,
                opened.base,
                opened.is_git,
                sources,
            );
            let config = self.config_value_with_project(
                &self.projects_dir,
                self.default_harness,
                self.isolation,
                Some(project.project()),
            );
            self.persist_config(&config)?;
            let reply = self.project_json(project.project());
            self.insert_project(project);
            Ok(reply)
        })();
        if value.is_err() {
            self.cleanup_unregistered_sources(created_checkouts);
        }
        value
    }

    /// Tests may directly settle a fresh preparation without transport metadata.
    #[cfg(test)]
    pub(in crate::app) fn append_project_sources(
        &mut self,
        project_id: &str,
        added: Vec<ProjectSource>,
        created_checkouts: Vec<PathBuf>,
    ) -> Result<Value, String> {
        let cloned_paths = created_checkouts.clone();
        self.append_project_sources_with_clones(project_id, added, created_checkouts, cloned_paths)
    }

    /// Append only after revalidating the current registry under the app lock.
    pub(in crate::app) fn append_project_sources_with_clones(
        &mut self,
        project_id: &str,
        added: Vec<ProjectSource>,
        created_checkouts: Vec<PathBuf>,
        cloned_paths: Vec<PathBuf>,
    ) -> Result<Value, String> {
        let value = self.commit_prepared_source_additions(project_id, added, &cloned_paths);
        if value.is_err() {
            self.cleanup_unregistered_sources(created_checkouts);
        } else {
            self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        }
        value
    }

    fn commit_prepared_source_additions(
        &mut self,
        project_id: &str,
        mut added: Vec<ProjectSource>,
        cloned_paths: &[PathBuf],
    ) -> Result<Value, String> {
        self.validate_prepared_sources(Some(project_id), &mut added, cloned_paths)?;
        let mut sources = self.project_for(project_id)?.sources.clone();
        self.commit_project_configuration(project_id, |projects| {
            // Provisional IDs are never published. Mint against the current
            // reservations in the same transaction as the membership edit.
            let ids = projects.allocate_source_ids(Some(project_id), added.len())?;
            for (source, id) in added.iter_mut().zip(ids) {
                source.id = id;
            }
            sources.extend(added);
            assert!(
                projects.set_sources(project_id, sources),
                "project was resolved"
            );
            Ok(())
        })
    }

    /// Legacy creation doors keep same-home dedupe, after the same validation.
    pub(in crate::app) fn register_opened_repository(
        &mut self,
        opened: crate::lifecycle::OpenedRepository,
    ) -> Result<Value, String> {
        let created: Vec<_> = opened.created_checkout.iter().cloned().collect();
        let value = self.register_prepared_repository(opened, &created);
        if value.is_err() {
            self.cleanup_unregistered_sources(created);
        }
        value
    }

    fn register_prepared_repository(
        &mut self,
        mut opened: crate::lifecycle::OpenedRepository,
        cloned_paths: &[PathBuf],
    ) -> Result<Value, String> {
        opened.path = canonical_source_path(&opened.path)?;
        self.validate_secret_source(&opened.path)?;
        self.refuse_source_directory_claim(&opened.path)?;
        if let Some(existing) = self.projects.find_by_canonical_path(&opened.path) {
            return Ok(self.project_json(existing));
        }
        let mut sources = [ProjectSource::added(
            "source-1".into(),
            "source".into(),
            "source".into(),
            opened.path.clone(),
            opened.is_git,
            opened.base.clone(),
        )];
        self.validate_prepared_sources(None, &mut sources, cloned_paths)?;
        let project = self.project_candidate(opened.path, opened.base, opened.is_git);
        let config = self.config_value_with_project(
            &self.projects_dir,
            self.default_harness,
            self.isolation,
            Some(project.project()),
        );
        self.persist_config(&config)?;
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
        let value = self.project_json(project);
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(value)
    }
}
