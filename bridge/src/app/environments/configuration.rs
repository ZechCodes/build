//! Prospective configuration is invisible while repository/volume probes run.
use crate::app::projects::{ProjectConfiguration, ProjectRegistry};
use crate::app::runtime::lifecycle::AppReservation;
use crate::app::snapshot_projection::{ProjectSnapshotRefresh, RenderedProjectSnapshotRefresh};
use crate::app::{AppState, DeferredWork};
use crate::isolation::Isolation;
use crate::lifecycle::{PendingRow, PendingState, Performed, WorktreeChange, WorktreeMutation};
use serde_json::Value;
use std::path::PathBuf;

type Finish = Box<dyn FnOnce(&mut AppState, Result<Value, String>) -> Result<Value, String> + Send>;
type Validate =
    Box<dyn FnOnce(&AppState, &RenderedProjectSnapshotRefresh) -> Result<(), String> + Send>;
struct ConfigurationChecks {
    validate: Validate,
    cloned_paths: Vec<PathBuf>,
}
struct ConfigurationProbe {
    refresh: ProjectSnapshotRefresh,
    environment: String,
    isolation: Option<Isolation>,
    repositories: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    cloned_paths: Vec<PathBuf>,
    #[cfg(test)]
    clone_gate: Option<crate::app::OffLockGate>,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}
struct ConfigurationObserved {
    refresh: RenderedProjectSnapshotRefresh,
    changed_roots: Vec<PathBuf>,
    _held: Vec<crate::source_sync::SyncLock>,
}
impl WorktreeMutation for ConfigurationProbe {
    type Output = ConfigurationObserved;
    fn perform(mut self) -> Result<Performed<Self::Output>, String> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        let held = if self.cloned_paths.is_empty() {
            vec![]
        } else {
            #[cfg(test)]
            if let Some(gate) = &self.clone_gate {
                gate.arrive();
            }
            crate::source_sync::SyncLock::acquire_many(
                &self.refresh.source_claim_paths(),
                crate::source_sync::SERVICE_FETCH_DEADLINE,
            )?
        };
        self.refresh
            .select_environment_edit(&self.environment, &self.repositories);
        let refresh = self.refresh.render();
        if let Some(isolation) = self.isolation {
            refresh.validate_environment_isolation(&self.environment, isolation)?;
        }
        let changed_roots = crate::app::snapshot_projection::affected_repository_roots(
            self.roots,
            &self.repositories,
        );
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: ConfigurationObserved {
                refresh,
                changed_roots,
                _held: held,
            },
        })
    }
}
struct ConfigurationSettlement {
    environment: String,
    previous: Value,
    live_projects: Vec<String>,
    prospective: ProjectConfiguration,
    finish: Finish,
    validate: Option<Validate>,
}
impl ConfigurationSettlement {
    fn settle(
        mut self,
        app: &mut AppState,
        result: Result<ConfigurationObserved, String>,
    ) -> Result<Value, String> {
        // Git facts survive a rejected configuration change. Publish compatible
        // evidence before staging, and retain it if validation or persistence fails.
        let result = result.and_then(|observed| {
            app.install_project_snapshot_refresh(observed.refresh.clone());
            self.commit(app, observed)
        });
        (self.finish)(app, result)
    }
    fn commit(
        &mut self,
        app: &mut AppState,
        observed: ConfigurationObserved,
    ) -> Result<Value, String> {
        // These paths were related off-lock. No Git discovery runs in settlement.
        app.invalidate_observed_repository_roots(&observed.changed_roots);
        if app.config_value(&app.projects_dir, app.default_harness, app.isolation) != self.previous
            || app.environment_project_ids(&self.environment) != self.live_projects
        {
            return Err("stale unversioned mutation: environment configuration changed while checking it; retry the edit".into());
        }
        self.validate
            .take()
            .expect("configuration validation runs once")(app, &observed.refresh)?;
        self.prospective.swap(&mut app.projects, &self.environment);
        let mut observations = app.capture_configuration_observations();
        app.install_project_snapshot_refresh(observed.refresh);
        self.prospective.restore_runtime_into(&mut app.projects);
        let row = app
            .environment_json(&self.environment)
            .expect("staged environment remains registered");
        let config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
        let projection = app.prepare_all_snapshot_projection();
        self.prospective.swap(&mut app.projects, &self.environment);
        app.swap_configuration_observations(&mut observations);
        if config != self.previous {
            app.persist_config(&config)?;
        }
        self.prospective.swap(&mut app.projects, &self.environment);
        app.swap_configuration_observations(&mut observations);
        if let Some(projection) = projection {
            app.publish_prepared_snapshot_projection(projection);
        }
        app.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(row)
    }
}
impl AppState {
    pub(in crate::app) fn commit_environment_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<Value, String> {
        self.commit_environment_configuration_with(id, change, None, vec![], |_, result| result)
    }
    pub(in crate::app) fn commit_environment_configuration_with(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
        isolation: Option<Isolation>,
        repositories: Vec<PathBuf>,
        finish: impl FnOnce(&mut AppState, Result<Value, String>) -> Result<Value, String>
            + Send
            + 'static,
    ) -> Result<Value, String> {
        self.commit_environment_configuration_checked(
            id,
            change,
            isolation,
            repositories,
            finish,
            |_| Ok(()),
        )
    }
    pub(in crate::app) fn commit_environment_configuration_checked(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
        isolation: Option<Isolation>,
        repositories: Vec<PathBuf>,
        finish: impl FnOnce(&mut AppState, Result<Value, String>) -> Result<Value, String>
            + Send
            + 'static,
        validate: impl FnOnce(&AppState) -> Result<(), String> + Send + 'static,
    ) -> Result<Value, String> {
        self.defer_environment_configuration(
            id,
            change,
            isolation,
            repositories,
            finish,
            ConfigurationChecks {
                validate: Box::new(move |app, _| validate(app)),
                cloned_paths: vec![],
            },
        )
    }

    pub(super) fn commit_environment_source_add(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
        cloned_paths: Vec<PathBuf>,
        finish: impl FnOnce(&mut AppState, Result<Value, String>) -> Result<Value, String>
            + Send
            + 'static,
        validate: impl FnOnce(&AppState, &RenderedProjectSnapshotRefresh) -> Result<(), String>
            + Send
            + 'static,
    ) -> Result<Value, String> {
        self.defer_environment_configuration(
            id,
            change,
            None,
            vec![],
            finish,
            ConfigurationChecks {
                validate: Box::new(validate),
                cloned_paths,
            },
        )
    }

    fn defer_environment_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
        isolation: Option<Isolation>,
        repositories: Vec<PathBuf>,
        finish: impl FnOnce(&mut AppState, Result<Value, String>) -> Result<Value, String>
            + Send
            + 'static,
        checks: ConfigurationChecks,
    ) -> Result<Value, String> {
        let prepared = self.prepare_environment_configuration(id, change);
        let (previous, prospective, mut refresh, row) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return finish(self, Err(error)),
        };
        let reservation = match self.reserve_row(row) {
            Ok(row) => AppReservation::row_only(row),
            Err(error) => return finish(self, Err(error)),
        };
        if !checks.cloned_paths.is_empty() {
            refresh = refresh.include_source_claims(self.projects.registered_sources());
        }
        let roots = self.capture_repository_roots();
        let probe = ConfigurationProbe {
            refresh,
            environment: id.to_owned(),
            isolation,
            repositories,
            roots,
            cloned_paths: checks.cloned_paths,
            #[cfg(test)]
            clone_gate: self.off_lock_environment_source_clone_gate.clone(),
            #[cfg(test)]
            gate: self.off_lock_environment_probe_gate.clone(),
        };
        let settlement = ConfigurationSettlement {
            environment: id.to_owned(),
            previous,
            live_projects: self.environment_project_ids(id),
            prospective,
            finish: Box::new(finish),
            validate: Some(checks.validate),
        };
        let job = self.lifecycle_job(reservation, probe, move |app: &mut AppState, result| {
            settlement.settle(app, result)
        });
        self.deferred_work = Some(DeferredWork::Lifecycle(Box::new(job)));
        Ok(Value::Null)
    }
    fn prepare_environment_configuration(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut ProjectRegistry) -> Result<(), String>,
    ) -> Result<
        (
            Value,
            ProjectConfiguration,
            ProjectSnapshotRefresh,
            PendingRow,
        ),
        String,
    > {
        self.mark_device_lifecycle_dirty()?;
        let environment = self
            .projects
            .environment(id)
            .ok_or_else(|| format!("unknown environment: {id}"))?;
        let home = environment
            .sources
            .first()
            .ok_or("environment has no home source")?
            .path
            .clone();
        let row = PendingRow::on_directory(
            crate::worktree::external_worktree_id(&home),
            environment.name.clone(),
            PendingState::Updating,
        );
        let previous = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let mut prospective = ProjectConfiguration::capture_environment(&self.projects, id);
        if let Err(error) = change(&mut self.projects) {
            prospective.swap(&mut self.projects, id);
            return Err(error);
        }
        self.projects.refresh_environment_projects(id);
        let refresh = self.capture_project_snapshot_refresh();
        prospective.swap(&mut self.projects, id);
        Ok((previous, prospective, refresh, row))
    }
}
