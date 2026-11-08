//! Owned observations installed with a revalidated configuration, without probes.
use super::*;

pub(in crate::app) struct ConfigurationObservations {
    projects: BTreeMap<String, ProjectObservation>,
    environments: BTreeMap<String, ProjectObservation>,
}
impl AppState {
    pub(in crate::app) fn capture_configuration_observations(&self) -> ConfigurationObservations {
        ConfigurationObservations {
            projects: self.snapshot_observations.projects.lock().unwrap().clone(),
            environments: self
                .snapshot_observations
                .environments
                .lock()
                .unwrap()
                .clone(),
        }
    }
    pub(in crate::app) fn swap_configuration_observations(
        &self,
        staged: &mut ConfigurationObservations,
    ) {
        std::mem::swap(
            &mut staged.projects,
            &mut self.snapshot_observations.projects.lock().unwrap(),
        );
        std::mem::swap(
            &mut staged.environments,
            &mut self.snapshot_observations.environments.lock().unwrap(),
        );
    }
    pub(in crate::app) fn install_environment_edit_refresh(
        &self,
        _environment: &str,
        refresh: RenderedProjectSnapshotRefresh,
    ) {
        self.install_project_snapshot_refresh(refresh);
    }
    pub(in crate::app) fn project_json_observed(&self, id: &str) -> Result<Value, String> {
        let row = self
            .project_list_rows()
            .into_iter()
            .find(|p| p.project_id == id)
            .ok_or_else(|| format!("unknown project: {id}"))?;
        let observations = self.snapshot_observations.projects.lock().unwrap();
        let observed = observations.get(id).map(|o| &o.value);
        let available = observed
            .map(|o| o["isolation_available"].clone())
            .unwrap_or_else(|| {
                json!(crate::isolation::IsolationAvailability::unavailable(
                    "repository observation pending"
                ))
            });
        let requested = self.project_isolation(self.project_for(id)?);
        let effective =
            if requested == crate::isolation::Isolation::Rift && available["rift"] != true {
                crate::isolation::Isolation::Worktree
            } else {
                requested
            };
        let mut value = row.render_observed(available, observed);
        value["isolation_effective"] = json!(effective);
        // Preserve the established mutation-row shape; list-only metadata stays
        // on project.list and its pushed snapshots.
        for name in [
            "environment_id",
            "environment_name",
            "project_key",
            "device",
            "open_tasks",
            "total_tasks",
            "workspaces",
            "entity_id",
            "run_id",
            "conversations",
        ] {
            value
                .as_object_mut()
                .expect("project row is an object")
                .remove(name);
        }
        Ok(value)
    }
    pub(in crate::app) fn invalidate_observed_repository_roots(&self, roots: &[PathBuf]) {
        for root in roots {
            self.invalidate_snapshot_checkout_observations(root);
            self.note_git_at(root);
        }
        if !roots.is_empty() {
            self.note_board_lists_changed(crate::changes::BoardLists {
                projects: true,
                workspaces: true,
                ..Default::default()
            });
        }
    }
}

impl RenderedProjectSnapshotRefresh {
    pub(in crate::app) fn validate_environment_isolation(
        &self,
        id: &str,
        isolation: crate::isolation::Isolation,
    ) -> Result<(), String> {
        let value = self
            .environments
            .iter()
            .find(|(row, _)| row.id == id)
            .map(|(_, value)| &value["isolation_available"])
            .ok_or("unknown environment")?;
        let rift = if value["rift"] == true {
            Ok(())
        } else {
            Err(value["reason"]
                .as_str()
                .unwrap_or("Rift is unavailable")
                .to_owned())
        };
        crate::app::config::accept_isolation(
            isolation.wire(),
            &crate::isolation::IsolationAvailability { rift },
        )
        .map(|_| ())
    }
}

impl ProjectSnapshotRefresh {
    /// Keep unrelated owners out of a mutation's expensive probe pass.
    pub(in crate::app) fn select_environment_edit(
        &mut self,
        environment: &str,
        repositories: &[PathBuf],
    ) {
        let repositories = repositories
            .iter()
            .map(|p| crate::source_sync::sync_lock_identity(p))
            .collect::<BTreeSet<_>>();
        let affected = |sources: &[crate::app::projects::ProjectSource]| {
            sources
                .iter()
                .any(|s| repositories.contains(&crate::source_sync::sync_lock_identity(&s.path)))
        };
        self.rows.retain(|p| {
            p.environment_id == environment || (!repositories.is_empty() && affected(&p.sources))
        });
        self.environments
            .retain(|e| e.id == environment || (!repositories.is_empty() && affected(&e.sources)));
    }
}

impl ProjectSnapshotRefresh {
    /// Complete a prospective project edit using filesystem results, then probe
    /// its future references before the app lock is acquired for settlement.
    pub(in crate::app) fn render_updated_source(
        mut self,
        project: &str,
        source: &str,
        updated: &crate::lifecycle::SourceUpdated,
        version: u64,
    ) -> RenderedProjectSnapshotRefresh {
        let environment = self
            .rows
            .iter()
            .find(|row| row.project_id == project)
            .map(|row| row.environment_id.clone())
            .expect("captured project");
        self.rows.retain(|row| row.environment_id == environment);
        self.environments.retain(|row| row.id == environment);
        for sources in self
            .rows
            .iter_mut()
            .map(|row| &mut row.sources)
            .chain(self.environments.iter_mut().map(|row| &mut row.sources))
        {
            for captured in sources
                .iter_mut()
                .filter(|s| s.id == source && s.path == updated.path)
            {
                captured.is_git = updated.is_git;
                captured.base_branch = updated.base_branch.clone();
            }
        }
        self.version = version;
        self.render()
    }
}
