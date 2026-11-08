//! `project.update_source` — edit one source in place: its label, its base
//! branch, its remote, and (past the first) the folder it stands on.
//!
//! Every client-named part is checked here, under the lock, before any git is
//! handed to the drain; the drain reads and proves the rest
//! ([`crate::lifecycle::UpdateSource`]) and the settlement writes the source's
//! record. Forward-looking like every project edit, with one exception the
//! user asked for: a workspace checkout with a repository of its own follows
//! a remote change when it still named the source's old `origin`.

use super::{canonical_source_path, ProjectSource, ResolvedProjectSource};
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{PendingState, SourceUpdated, UpdateSource, WorkspaceCheckout};
use crate::remote_url::usable_remote_url;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Longer than any label a person types; a cap so nothing unbounded is stored.
const MAX_LABEL_LEN: usize = 200;

#[cfg(test)]
mod settlement_tests;

/// The parts of a source an edit names, each checked.
pub(in crate::app) struct SourceEdit {
    pub(in crate::app) name: Option<String>,
    pub(in crate::app) path: Option<PathBuf>,
    pub(in crate::app) base_branch: Option<String>,
    pub(in crate::app) remote: Option<String>,
    /// Whether the base is kept in step with its remote (#267). Needs no git,
    /// so an edit naming only this is written at once.
    pub(in crate::app) sync_base: Option<bool>,
}

impl SourceEdit {
    pub(in crate::app) fn needs_git(&self) -> bool {
        self.path.is_some() || self.base_branch.is_some() || self.remote.is_some()
    }

    fn is_empty(&self) -> bool {
        self.name.is_none() && !self.needs_git() && self.sync_base.is_none()
    }
}

/// Where the settlement finds the source again, and what it writes there.
struct SourceAddress {
    project_id: String,
    source_id: String,
    path: PathBuf,
    name: Option<String>,
    sync_base: Option<bool>,
}

impl AppState {
    pub(crate) fn project_update_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let project_id = require_str(params, "project_id")?;
        let source_id = require_str(params, "source_id")?;
        self.check_project_source_edit(&project_id, &source_id, true)?;
        let project = self.project_for(&project_id)?;
        let (index, source) = project
            .sources
            .iter()
            .enumerate()
            .find(|(_, source)| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        let source = source.clone();
        let title = project.name.clone();
        let edit = self.checked_edit(params, &project_id, &source, index)?;
        if edit
            .path
            .as_ref()
            .is_some_and(|path| *path != Self::canonical_root(&source.path))
        {
            self.refuse_source_shells_at(&source.path)?;
        }
        if !edit.needs_git() && edit.name.is_none() {
            return self.set_source_sync(&project_id, &source_id, edit.sync_base);
        }
        let moves = edit.path.as_ref().is_some_and(|path| *path != source.path);
        if moves {
            self.refuse_moving_sources_under_workers(&project_id)?;
        }
        let mutation = UpdateSource {
            path: source.path.clone(),
            is_git: source.is_git,
            base_branch: source.base_branch.clone(),
            new_path: edit.path,
            new_base_branch: edit.base_branch,
            new_remote: edit.remote,
            workspace_checkouts: self.checkouts_cut_from(&project_id, &source),
        };
        let address = SourceAddress {
            project_id,
            source_id,
            path: source.path.clone(),
            name: edit.name,
            sync_base: edit.sync_base,
        };
        let captured = self.resolve_project_source(
            &address.project_id,
            &address.source_id,
            None,
            super::MOVED_GIT_SOURCE,
        )?;
        let prospective =
            self.capture_prospective_project_refresh(&address.project_id, |projects| {
                projects.update_source(
                    &address.project_id,
                    &address.source_id,
                    &address.path,
                    super::SourceRecord {
                        name: address.name.clone(),
                        path: mutation
                            .new_path
                            .clone()
                            .unwrap_or_else(|| source.path.clone()),
                        is_git: source.is_git,
                        base_branch: mutation
                            .new_base_branch
                            .clone()
                            .unwrap_or_else(|| source.base_branch.clone()),
                        sync_base: address.sync_base,
                    },
                )
            })?;
        let environments = self.capture_repository_refresh();
        let mutation = SerializedSourceUpdate {
            mutation,
            captured: SourceUpdateScope::Project(captured),
            state: self.source_validation_handle(),
            environments: Some(environments.clone()),
            configuration_probe: Some((prospective, self.read_view.clone())),
        };
        let project_id = address.project_id.clone();
        let deferred = self.defer_project(
            source.path,
            title,
            PendingState::Updating,
            mutation,
            move |state: &mut AppState, result: Result<LockedSourceUpdate, String>| {
                environments.install(state);
                if moves {
                    state.source_moves.end(&address.project_id);
                }
                let LockedSourceUpdate {
                    updated,
                    _held,
                    configuration_observed,
                } = result?;
                state.settle_source_update(address, updated, configuration_observed)
            },
        )?;
        // Admission and this hold are acquired under the same app mutex.
        // A rejected request owns no hold; only this move's settlement ends it.
        if moves {
            self.source_moves.begin(&project_id);
        }
        Ok(deferred)
    }

    fn checked_edit(
        &self,
        params: &Value,
        project_id: &str,
        source: &ProjectSource,
        index: usize,
    ) -> Result<SourceEdit, String> {
        let path = optional_text(params, "path")
            .map(|path| self.usable_source_move(project_id, source, index, path))
            .transpose()?;
        checked_source_edit(params, source, path)
    }

    /// The folder a later source moves to, held to what adding a source is
    /// held to: it exists, it is a directory, and it neither holds nor sits
    /// inside any other registered source.
    fn usable_source_move(
        &self,
        project_id: &str,
        source: &ProjectSource,
        index: usize,
        path: &str,
    ) -> Result<PathBuf, String> {
        let candidate = canonical_source_path(&expand_tilde(path.trim()))?;
        self.validate_secret_source(&candidate)?;
        self.refuse_source_directory_claim(&candidate)?;
        if candidate == source.path {
            return Ok(candidate);
        }
        if index == 0 {
            return Err(
                "The first folder is the project's home and cannot move. Add the other folder as a source instead."
                    .to_string(),
            );
        }
        self.refuse_source_duplicate(project_id, &candidate, &source.id)?;
        self.refuse_source_containment(&candidate, Some((project_id, &source.id)))?;
        Ok(candidate)
    }

    /// Every workspace directory cut from this source, other than the source's
    /// own folder (an adopted workspace stands on it).
    fn checkouts_cut_from(
        &self,
        project_id: &str,
        source: &ProjectSource,
    ) -> Vec<WorkspaceCheckout> {
        self.workspaces
            .list(Some(project_id))
            .into_iter()
            .flat_map(|workspace| {
                workspace
                    .directories
                    .iter()
                    .map(move |directory| (&workspace.id, directory))
            })
            .filter(|(_, directory)| {
                directory.source_id == source.id && directory.path != source.path
            })
            .map(|(workspace_id, directory)| WorkspaceCheckout {
                workspace_id: workspace_id.clone(),
                path: directory.path.clone(),
            })
            .collect()
    }

    fn settle_source_update(
        &mut self,
        address: SourceAddress,
        updated: SourceUpdated,
        observed: Option<crate::app::snapshot_projection::RenderedProjectSnapshotRefresh>,
    ) -> Result<Value, String> {
        self.validate_settled_source_update(&address, &updated)?;
        let checkouts_updated = updated.checkouts_updated;
        let checkouts_failed: Vec<Value> = updated
            .checkouts_failed
            .iter()
            .map(|failed| {
                json!({
                    "workspace_id": failed.workspace_id,
                    "path": failed.path.display().to_string(),
                    "reason": failed.reason,
                })
            })
            .collect();
        let mut row = self.commit_project_configuration_with_observation(
            &address.project_id,
            |projects| {
                projects.update_source(
                    &address.project_id,
                    &address.source_id,
                    &address.path,
                    super::SourceRecord {
                        name: address.name,
                        path: updated.path,
                        is_git: updated.is_git,
                        base_branch: updated.base_branch,
                        sync_base: address.sync_base,
                    },
                )
            },
            observed,
        )?;
        if address.sync_base == Some(true) {
            self.request_source_sync(address.project_id.clone(), address.source_id.clone());
        }
        row["checkouts_updated"] = checkouts_updated.into();
        row["checkouts_failed"] = checkouts_failed.into();
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(row)
    }

    /// The directory can gain another reference while the filesystem work
    /// runs. Only the current registry, under settlement's app lock, decides
    /// whether this edit can publish its resulting location.
    fn validate_settled_source_update(
        &self,
        address: &SourceAddress,
        updated: &SourceUpdated,
    ) -> Result<(), String> {
        let candidate = canonical_source_path(&updated.path)?;
        if candidate != updated.path {
            return Err("source directory changed while its edit was being prepared".into());
        }
        self.validate_secret_source(&candidate)?;
        self.refuse_source_directory_claim(&candidate)?;
        self.refuse_source_duplicate(&address.project_id, &candidate, &address.source_id)?;
        self.refuse_source_containment(&candidate, Some((&address.project_id, &address.source_id)))
    }
}

impl AppState {
    /// Turn keeping a source's base in step on or off: no git, so written at
    /// once. Turning it on syncs the source now; turning it off leaves it out
    /// of every pass from the next one on.
    fn set_source_sync(
        &mut self,
        project_id: &str,
        source_id: &str,
        sync_base: Option<bool>,
    ) -> Result<Value, String> {
        let mut row = self.commit_project_configuration(project_id, |projects| {
            let source = projects
                .source_mut(project_id, source_id)
                .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
            source.sync_base = sync_base;
            Ok(())
        })?;
        if sync_base == Some(true) {
            self.request_source_sync(project_id.to_string(), source_id.to_string());
        }
        row["checkouts_updated"] = 0.into();
        row["checkouts_failed"] = json!([]);
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(row)
    }
}

fn optional_text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// Remote edits mutate the checkout and independent copies that follow it.
/// Other reference edits change only configuration.
pub(in crate::app) fn remote_mutation_paths(mutation: &UpdateSource) -> Vec<PathBuf> {
    if mutation.new_remote.is_none() {
        return Vec::new();
    }
    std::iter::once(mutation.path.clone())
        .chain(mutation.new_path.iter().cloned())
        .chain(
            mutation
                .workspace_checkouts
                .iter()
                .map(|checkout| checkout.path.clone()),
        )
        .collect()
}

fn usable_label(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A folder's label cannot be empty.".to_string());
    }
    if name.len() > MAX_LABEL_LEN || name.chars().any(char::is_control) {
        return Err("A folder's label must be one short line.".to_string());
    }
    Ok(name.to_string())
}

/// A checked url, or empty for "take `origin` off".
fn usable_remote_or_clear(remote: &str) -> Result<String, String> {
    match remote.trim() {
        "" => Ok(String::new()),
        named => usable_remote_url(named),
    }
}

/// Source settings and their Git writes take one turn together. Keep the old,
/// new and independent workspace repository locks through config settlement.
pub(in crate::app) struct SerializedSourceUpdate {
    pub(in crate::app) mutation: UpdateSource,
    pub(in crate::app) captured: SourceUpdateScope,
    pub(in crate::app) state: Option<std::sync::Weak<std::sync::Mutex<AppState>>>,
    pub(in crate::app) environments: Option<crate::app::snapshot_projection::RepositoryRefresh>,
    pub(in crate::app) configuration_probe: Option<(
        crate::app::snapshot_projection::ProjectSnapshotRefresh,
        crate::app::read_view::ReadViewSlot,
    )>,
}

pub(in crate::app) struct LockedSourceUpdate {
    pub(in crate::app) updated: SourceUpdated,
    _held: Vec<crate::source_sync::SyncLock>,
    configuration_observed: Option<crate::app::snapshot_projection::RenderedProjectSnapshotRefresh>,
}

impl crate::lifecycle::WorktreeMutation for SerializedSourceUpdate {
    type Output = LockedSourceUpdate;

    fn perform(self) -> Result<crate::lifecycle::Performed<Self::Output>, String> {
        use crate::source_sync::{SyncLock, SERVICE_FETCH_DEADLINE};
        let mut paths = vec![self.mutation.path.clone()];
        paths.extend(self.mutation.new_path.iter().cloned());
        paths.extend(
            self.mutation
                .workspace_checkouts
                .iter()
                .map(|checkout| checkout.path.clone()),
        );
        let held = SyncLock::acquire_many(&paths, SERVICE_FETCH_DEADLINE)?;
        if let Some(state) = self.state.as_ref().and_then(std::sync::Weak::upgrade) {
            let app = state.lock().unwrap();
            self.captured
                .validate(&app, self.mutation.new_path.as_deref())?;
        }
        let project_source = match &self.captured {
            SourceUpdateScope::Project(source) => {
                Some((source.project_id.clone(), source.source_id.clone()))
            }
            SourceUpdateScope::Environment { .. } => None,
        };
        let performed = self.mutation.perform();
        if let Some(refresh) = &self.environments {
            if let Some(state) = self.state.as_ref().and_then(std::sync::Weak::upgrade) {
                refresh.recapture(&state.lock().unwrap());
            }
            refresh.observe(&paths);
        }
        let performed = performed?;
        let configuration_observed = self.configuration_probe.zip(project_source).map(
            |((refresh, clock), (project, source))| {
                refresh.render_updated_source(
                    &project,
                    &source,
                    &performed.output,
                    clock.next_version(),
                )
            },
        );
        Ok(crate::lifecycle::Performed {
            change: performed.change,
            output: LockedSourceUpdate {
                updated: performed.output,
                _held: held,
                configuration_observed,
            },
        })
    }
}

pub(in crate::app) fn checked_source_edit(
    params: &Value,
    source: &ProjectSource,
    path: Option<PathBuf>,
) -> Result<SourceEdit, String> {
    let edit = SourceEdit {
        name: optional_text(params, "name")
            .map(usable_label)
            .transpose()?,
        path,
        base_branch: optional_text(params, "base_branch").map(|branch| branch.trim().to_string()),
        remote: optional_text(params, "remote")
            .map(usable_remote_or_clear)
            .transpose()?,
        sync_base: params.get("sync_base").and_then(Value::as_bool),
    };
    if edit.is_empty() {
        return Err(
            "Name a label, path, base branch, remote or sync setting to change.".to_string(),
        );
    }
    if edit.sync_base.is_some() && !source.is_git {
        return Err(
            "A folder that is not a Git repository has no base branch to keep up to date."
                .to_string(),
        );
    }
    Ok(edit)
}

/// Revalidation names the actual reference owner; an environment needs no project proxy.
pub(in crate::app) enum SourceUpdateScope {
    Project(ResolvedProjectSource),
    Environment { id: String, source: ProjectSource },
}
impl SourceUpdateScope {
    fn validate(&self, app: &AppState, candidate: Option<&std::path::Path>) -> Result<(), String> {
        match self {
            Self::Project(captured) => {
                app.validate_source_location(captured)?;
                if let Some(candidate) = candidate {
                    app.refuse_source_directory_claim(candidate)?;
                    app.refuse_source_duplicate(
                        &captured.project_id,
                        candidate,
                        &captured.source_id,
                    )?;
                    app.refuse_source_containment(
                        candidate,
                        Some((&captured.project_id, &captured.source_id)),
                    )?;
                }
            }
            Self::Environment { id, source } => {
                let current = app.environment_source(id, &source.id)?;
                if current.path != source.path || current.is_git != source.is_git {
                    return Err(super::MOVED_GIT_SOURCE.into());
                }
                if let Some(candidate) = candidate {
                    app.validate_environment_source_path(id, Some(&source.id), candidate)?;
                }
            }
        }
        Ok(())
    }
}

impl LockedSourceUpdate {
    pub(in crate::app) fn into_parts(self) -> (SourceUpdated, Vec<crate::source_sync::SyncLock>) {
        (self.updated, self._held)
    }
}
