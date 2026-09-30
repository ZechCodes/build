//! `project.update_source` — edit one source in place: its label, its base
//! branch, its remote, and (past the first) the folder it stands on.
//!
//! Every client-named part is checked here, under the lock, before any git is
//! handed to the drain; the drain reads and proves the rest
//! ([`crate::lifecycle::UpdateSource`]) and the settlement writes the source's
//! record. Forward-looking like every project edit, with one exception the
//! user asked for: a workspace checkout with a repository of its own follows
//! a remote change when it still named the source's old `origin`.

use super::{canonical_source_path, ProjectSource};
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{PendingState, SourceUpdated, UpdateSource, WorkspaceCheckout};
use crate::remote_url::usable_remote_url;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Longer than any label a person types; a cap so nothing unbounded is stored.
const MAX_LABEL_LEN: usize = 200;

/// The parts of a source an edit names, each checked.
struct SourceEdit {
    name: Option<String>,
    path: Option<PathBuf>,
    base_branch: Option<String>,
    remote: Option<String>,
    /// Whether the base is kept in step with its remote (#267). Needs no git,
    /// so an edit naming only this is written at once.
    sync_base: Option<bool>,
}

impl SourceEdit {
    fn needs_git(&self) -> bool {
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
        let project_id = require_str(params, "project_id")?;
        let source_id = require_str(params, "source_id")?;
        let project = self.project_for(&project_id)?;
        let (index, source) = project
            .sources
            .iter()
            .enumerate()
            .find(|(_, source)| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        let source = source.clone();
        let title = project.name.clone();
        let edit = self.checked_edit(params, &source, index)?;
        if !edit.needs_git() && edit.name.is_none() {
            return self.set_source_sync(&project_id, &source_id, edit.sync_base);
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
        self.defer_project(
            source.path,
            title,
            PendingState::Updating,
            mutation,
            move |state: &mut AppState, result: Result<SourceUpdated, String>| {
                state.settle_source_update(address, result?)
            },
        )
    }

    fn checked_edit(
        &self,
        params: &Value,
        source: &ProjectSource,
        index: usize,
    ) -> Result<SourceEdit, String> {
        let edit = SourceEdit {
            name: optional_text(params, "name")
                .map(usable_label)
                .transpose()?,
            path: optional_text(params, "path")
                .map(|path| self.usable_source_move(source, index, path))
                .transpose()?,
            base_branch: optional_text(params, "base_branch")
                .map(|branch| branch.trim().to_string()),
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

    /// The folder a later source moves to, held to what adding a source is
    /// held to: it exists, it is a directory, and it neither holds nor sits
    /// inside any other registered source.
    fn usable_source_move(
        &self,
        source: &ProjectSource,
        index: usize,
        path: &str,
    ) -> Result<PathBuf, String> {
        let candidate = canonical_source_path(&expand_tilde(path.trim()))?;
        if candidate == source.path {
            return Ok(candidate);
        }
        if index == 0 {
            return Err(
                "The first folder is the project's home and cannot move. Add the other folder as a source instead."
                    .to_string(),
            );
        }
        let overlaps = self
            .projects
            .iter()
            .flat_map(|project| &project.sources)
            .filter(|other| other.path != source.path)
            .any(|other| candidate.starts_with(&other.path) || other.path.starts_with(&candidate));
        if overlaps {
            return Err(format!(
                "source overlaps a registered project source: {}",
                candidate.display()
            ));
        }
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
    ) -> Result<Value, String> {
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
        self.projects.update_source(
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
        )?;
        self.persist();
        if address.sync_base == Some(true) {
            self.request_source_sync(address.project_id.clone(), address.source_id.clone());
        }
        let project = self
            .projects
            .get(&address.project_id)
            .expect("the project was just updated");
        let mut row = self.project_json(project);
        row["checkouts_updated"] = checkouts_updated.into();
        row["checkouts_failed"] = checkouts_failed.into();
        Ok(row)
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
        let source = self
            .projects
            .source_mut(project_id, source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        source.sync_base = sync_base;
        self.persist();
        if sync_base == Some(true) {
            self.request_source_sync(project_id.to_string(), source_id.to_string());
        }
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        let project = self.project_for(project_id)?;
        let mut row = self.project_json(project);
        row["checkouts_updated"] = 0.into();
        row["checkouts_failed"] = json!([]);
        Ok(row)
    }
}

fn optional_text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
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
