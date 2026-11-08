//! Source addition discovers origins through the common off-lock renderer.
use crate::app::projects::requests::{PreparedSources, TakenSourceNames};
use crate::app::projects::ProjectSource;
use crate::app::snapshot_projection::{ProjectSnapshotRefresh, RenderedProjectSnapshotRefresh};
use crate::app::{require_str, AppState};
use crate::lifecycle::{PendingState, Performed, WorktreeChange, WorktreeMutation};
use serde_json::Value;
use std::path::PathBuf;

struct SourceAddScope(Value);
impl SourceAddScope {
    fn capture(app: &AppState) -> Self {
        Self(app.config_value(&app.projects_dir, app.default_harness, app.isolation))
    }
    fn validate(&self, app: &AppState) -> Result<(), String> {
        if self.0 != app.config_value(&app.projects_dir, app.default_harness, app.isolation) {
            return Err("stale unversioned mutation: source membership or name claims changed; retry the edit".into());
        }
        Ok(())
    }
}

struct AdmissionProbe {
    refresh: ProjectSnapshotRefresh,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}
impl WorktreeMutation for AdmissionProbe {
    type Output = RenderedProjectSnapshotRefresh;
    fn perform(self) -> Result<Performed<Self::Output>, String> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: self.refresh.render(),
        })
    }
}

impl AppState {
    pub(crate) fn environment_add_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let id = require_str(params, "environment_id")?;
        self.refuse_pending_environment_creation(&id)?;
        let environment = self
            .projects
            .environment(&id)
            .ok_or_else(|| format!("unknown environment: {id}"))?;
        let remote = params
            .get("remote")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty());
        if !remote {
            return self.begin_environment_source_open(&id, params, None);
        }
        let home = environment
            .sources
            .first()
            .ok_or("environment has no home source")?
            .path
            .clone();
        let title = environment.name.clone();
        let scope = SourceAddScope::capture(self);
        let probe = AdmissionProbe {
            refresh: self
                .capture_project_snapshot_refresh()
                .for_source_claims(self.projects.registered_sources()),
            #[cfg(test)]
            gate: self.off_lock_environment_source_admission_gate.clone(),
        };
        let params = params.clone();
        self.defer_project(
            home,
            title,
            PendingState::Updating,
            probe,
            move |app: &mut AppState, result: Result<RenderedProjectSnapshotRefresh, String>| {
                let observed = result?;
                scope.validate(app)?;
                app.begin_environment_source_open(&id, &params, Some(&observed))
            },
        )
    }

    fn begin_environment_source_open(
        &mut self,
        id: &str,
        params: &Value,
        observed: Option<&RenderedProjectSnapshotRefresh>,
    ) -> Result<Value, String> {
        let title = self
            .projects
            .environment(id)
            .ok_or("unknown environment")?
            .name
            .clone();
        let references = self.projects.environment_reference_sources(id, None);
        let registered = self.projects.registered_sources();
        let origins = |sources: &[ProjectSource]| -> Result<Vec<String>, String> {
            match observed {
                Some(facts) => sources
                    .iter()
                    .map(|s| self.source_claim_remote(facts, s))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| values.into_iter().flatten().collect()),
                None => Ok(vec![]),
            }
        };
        let mut taken = TakenSourceNames::with_origins(&references, origins(&references)?);
        let registered_origins = origins(&registered)?;
        let source_id = self
            .projects
            .allocate_environment_source_ids(id, 1)?
            .remove(0);
        let request =
            self.source_request_with_origin_check(params, source_id, &mut taken, |remote| {
                registered_origins
                    .iter()
                    .any(|origin| crate::worktree::remotes_match(origin, remote))
            })?;
        let managed = self.projects_dir.join(format!("{id}-sources"));
        taken.refuse_managed_destinations(std::slice::from_ref(&request), &managed, &registered)?;
        let dest = request
            .path
            .clone()
            .unwrap_or_else(|| managed.join(&request.mount));
        let mutation = self.reserve_source_opening(vec![request], managed)?;
        let scope = SourceAddScope::capture(self);
        let id = id.to_owned();
        self.defer_project(
            dest,
            title,
            PendingState::Updating,
            mutation,
            move |app: &mut AppState, result: Result<PreparedSources, String>| {
                let prepared = result?;
                if let Err(error) = scope.validate(app) {
                    app.cleanup_unregistered_sources(prepared.created_checkouts);
                    return Err(error);
                }
                app.append_environment_sources(&id, prepared)
            },
        )
    }

    fn append_environment_sources(
        &mut self,
        id: &str,
        mut prepared: PreparedSources,
    ) -> Result<Value, String> {
        let mut sources = std::mem::take(&mut prepared.sources);
        let validation = (|| {
            self.projects.environment(id).ok_or("unknown environment")?;
            self.validate_prepared_source_paths(&mut sources, &mut TakenSourceNames::default())?;
            for source in &sources {
                self.validate_environment_source_path(id, None, &source.path)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = validation {
            self.cleanup_unregistered_sources(prepared.created_checkouts);
            return Err(error);
        }
        let cloned = prepared.cloned_paths.clone();
        self.commit_environment_source_add(
            id,
            |projects| {
                let ids = projects.allocate_environment_source_ids(id, sources.len())?;
                for (source, id) in sources.iter_mut().zip(ids) {
                    source.id = id;
                }
                let environment = projects.environment_mut(id).ok_or("unknown environment")?;
                environment.next_source = environment
                    .next_source
                    .max(crate::app::projects::next_source_number(&sources));
                environment.sources.extend(sources);
                Ok(())
            },
            cloned.clone(),
            move |app, result| {
                if result.is_err() {
                    app.cleanup_unregistered_sources(std::mem::take(
                        &mut prepared.created_checkouts,
                    ));
                }
                drop(prepared);
                result
            },
            move |app, facts| app.validate_environment_clone_claims(&cloned, facts),
        )
    }

    fn validate_environment_clone_claims(
        &self,
        cloned: &[PathBuf],
        facts: &RenderedProjectSnapshotRefresh,
    ) -> Result<(), String> {
        if cloned.is_empty() {
            return Ok(());
        }
        let candidates = facts
            .source_claim_inputs()
            .filter(|s| {
                cloned
                    .iter()
                    .any(|p| crate::worktree::canonical_planned_path(p) == s.path)
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err("stale unversioned mutation: clone origin observations missing".into());
        }
        let registered = self.projects.registered_sources();
        for source in &candidates {
            if let Some(remote) = self.source_claim_remote(facts, source)? {
                for other in registered
                    .iter()
                    .chain(candidates.iter().copied())
                    .filter(|other| other.path != source.path)
                {
                    if self
                        .source_claim_remote(facts, other)?
                        .is_some_and(|origin| crate::worktree::remotes_match(&origin, &remote))
                    {
                        return Err(format!("source remote is already registered: {remote}"));
                    }
                }
            }
        }
        Ok(())
    }
}
