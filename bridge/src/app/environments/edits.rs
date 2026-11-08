//! Display names and forward-looking source membership on an environment.
use crate::app::projects::requests::{PreparedSources, TakenSourceNames};
use crate::app::projects::{canonical_source_path, ProjectSource};
use crate::app::{require_str, AppState};
use crate::lifecycle::PendingState;
use serde_json::Value;
use std::path::Path;

pub(super) fn display_name(name: &str) -> Result<String, String> {
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err("A name must be one bounded line (at most 200 bytes).".into());
    }
    let name = name.trim();
    if name.is_empty() || name.len() > 200 {
        return Err("A name must be one bounded line (at most 200 bytes).".into());
    }
    Ok(name.to_owned())
}

impl AppState {
    pub(crate) fn project_rename(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "project_id")?;
        let name = display_name(&require_str(params, "name")?)?;
        let environment = self.project_for(&id)?.environment_id.clone();
        let result_id = id.clone();
        self.commit_environment_configuration_with(
            &environment,
            |projects| projects.rename_project(&id, name),
            None,
            vec![],
            move |app, result| {
                result?;
                app.project_json_observed(&result_id)
            },
        )
    }
    pub(crate) fn environment_rename(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "environment_id")?;
        let name = display_name(&require_str(params, "name")?)?;
        self.commit_environment_configuration(&id, |projects| {
            projects
                .environment_mut(&id)
                .ok_or("unknown environment")?
                .name = name;
            Ok(())
        })
    }
    pub(in crate::app) fn environment_json(&self, id: &str) -> Result<Value, String> {
        self.environment_list()["environments"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["environment_id"] == id)
            .cloned()
            .ok_or_else(|| format!("unknown environment: {id}"))
    }
    pub(in crate::app) fn environment_source(
        &self,
        id: &str,
        source_id: &str,
    ) -> Result<&ProjectSource, String> {
        self.projects
            .environment(id)
            .ok_or_else(|| format!("unknown environment: {id}"))?
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in environment {id}"))
    }
    pub(in crate::app) fn environment_project_ids(&self, id: &str) -> Vec<String> {
        self.projects
            .iter()
            .filter(|p| p.environment_id == id)
            .map(|p| p.id.clone())
            .collect()
    }
    pub(in crate::app) fn validate_environment_source_path(
        &self,
        id: &str,
        source_id: Option<&str>,
        path: &Path,
    ) -> Result<(), String> {
        let candidate = canonical_source_path(path)?;
        if candidate != path {
            return Err("source directory changed while its edit was being prepared".into());
        }
        self.validate_secret_source(&candidate)?;
        self.refuse_source_directory_claim(&candidate)?;
        let effective = self.projects.environment_reference_sources(id, source_id);
        if effective.iter().any(|s| s.path == candidate) {
            return Err(format!(
                "source is already included in this project: {}",
                candidate.display()
            ));
        }
        let registered = source_id.map_or_else(
            || self.projects.registered_sources(),
            |source| {
                self.projects
                    .registered_except_environment_source(id, source)
            },
        );
        if registered.iter().any(|s| {
            s.path != candidate
                && (s.path.starts_with(&candidate) || candidate.starts_with(&s.path))
        }) {
            return Err(format!(
                "source overlaps a registered project source: {}",
                candidate.display()
            ));
        }
        Ok(())
    }
    pub(in crate::app) fn refuse_environment_source_move(
        &self,
        id: &str,
        source: &ProjectSource,
    ) -> Result<(), String> {
        crate::app::fs::refuse_active_source(&source.path)?;
        self.refuse_source_shells_at(&source.path)?;
        for project in self.environment_project_ids(id) {
            self.refuse_moving_sources_under_workers(&project)?;
        }
        Ok(())
    }
    pub(crate) fn environment_add_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let id = require_str(params, "environment_id")?;
        self.refuse_pending_environment_creation(&id)?;
        let environment = self
            .projects
            .environment(&id)
            .ok_or_else(|| format!("unknown environment: {id}"))?;
        let title = environment.name.clone();
        let source_id = self
            .projects
            .allocate_environment_source_ids(&id, 1)?
            .remove(0);
        let mut taken =
            TakenSourceNames::over(&self.projects.environment_reference_sources(&id, None));
        let request = self.source_request(params, source_id, &mut taken)?;
        // Stable identity, independent of the environment's display name.
        let managed = self.projects_dir.join(format!("{id}-sources"));
        taken.refuse_managed_destinations(
            std::slice::from_ref(&request),
            &managed,
            &self.projects.registered_sources(),
        )?;
        let dest = request
            .path
            .clone()
            .unwrap_or_else(|| managed.join(&request.mount));
        let mutation = self.reserve_source_opening(vec![request], managed)?;
        self.defer_project(
            dest,
            title,
            PendingState::Updating,
            mutation,
            move |state: &mut AppState, prepared: Result<PreparedSources, String>| {
                state.append_environment_sources(&id, prepared?)
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
            self.validate_prepared_sources(None, &mut sources, &prepared.cloned_paths)?;
            for source in &sources {
                self.validate_environment_source_path(id, None, &source.path)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = validation {
            self.cleanup_unregistered_sources(prepared.created_checkouts);
            return Err(error);
        }
        self.commit_environment_configuration_with(
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
            None,
            vec![],
            move |app, result| {
                if result.is_err() {
                    app.cleanup_unregistered_sources(std::mem::take(
                        &mut prepared.created_checkouts,
                    ));
                }
                drop(prepared);
                result
            },
        )
    }

    pub(crate) fn environment_remove_source(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "environment_id")?;
        let source_id = require_str(params, "source_id")?;
        let source = self.environment_source(&id, &source_id)?;
        if self.projects.environment(&id).unwrap().sources.len() == 1 {
            return Err("an environment must have at least one source".into());
        }
        self.refuse_environment_source_move(&id, source)?;
        let held = self.environment_project_ids(&id);
        for project in &held {
            self.source_moves.begin(project);
        }
        let check_id = id.clone();
        let check_source = source_id.clone();
        self.commit_environment_configuration_checked(
            &id,
            |projects| {
                projects
                    .environment_mut(&id)
                    .ok_or("unknown environment")?
                    .sources
                    .retain(|s| s.id != source_id);
                Ok(())
            },
            None,
            vec![],
            move |app, result| {
                for project in held {
                    app.source_moves.end(&project);
                }
                result
            },
            move |app| {
                app.refuse_environment_source_move(
                    &check_id,
                    app.environment_source(&check_id, &check_source)?,
                )
            },
        )
    }

    pub(crate) fn environment_set_isolation(&mut self, params: &Value) -> Result<Value, String> {
        let id = require_str(params, "environment_id")?;
        self.projects
            .environment(&id)
            .ok_or_else(|| format!("unknown environment: {id}"))?;
        let isolation = match params.get("isolation") {
            Some(Value::Null) => None,
            _ => {
                let named = require_str(params, "isolation")?;
                Some(
                    crate::isolation::Isolation::from_wire(&named).ok_or_else(|| {
                        format!("unknown isolation {named:?} (expected \"worktree\" or \"rift\")")
                    })?,
                )
            }
        };
        self.commit_environment_configuration_with(
            &id,
            |projects| {
                projects
                    .environment_mut(&id)
                    .ok_or("unknown environment")?
                    .isolation = isolation;
                Ok(())
            },
            isolation,
            vec![],
            |_, result| result,
        )
    }
}
