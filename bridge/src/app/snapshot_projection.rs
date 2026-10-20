//! Producer-owned observations and immutable wire projections. Expensive probes
//! are captured by lifecycle work; publication consumes only owned observations.

use super::read_view::{ReadView, SnapshotDelta};
use super::{AppState, ProjectListRow};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(in crate::app) struct SnapshotObservations {
    ready: bool,
    pending_owners: Mutex<BTreeSet<String>>,
    paths: BTreeMap<PathBuf, PathBuf>,
    existing_paths: BTreeSet<PathBuf>,
    branches: BTreeMap<String, String>,
    watched_tasks: Mutex<BTreeMap<String, Vec<Value>>>,
    projects: Mutex<BTreeMap<String, ProjectObservation>>,
    workspace_reviews: BTreeMap<String, Value>,
}

pub(in crate::app) struct ProjectObservation {
    version: u64,
    value: Value,
}

pub(in crate::app) struct ProjectSnapshotRefresh {
    version: u64,
    rows: Vec<ProjectListRow>,
}

pub(in crate::app) struct RenderedProjectSnapshotRefresh {
    version: u64,
    rows: Vec<(ProjectListRow, Value)>,
}

impl ProjectSnapshotRefresh {
    pub(in crate::app) fn render(self) -> RenderedProjectSnapshotRefresh {
        RenderedProjectSnapshotRefresh {
            version: self.version,
            rows: self
                .rows
                .into_iter()
                .map(|row| {
                    let value = row.render();
                    (row, value)
                })
                .collect(),
        }
    }
}

impl AppState {
    pub(in crate::app) fn snapshot_projection_initialized(&self) -> bool {
        self.snapshot_observations.ready
    }

    /// Warm before the shared app starts accepting clients. A first immutable
    /// board is built from the restored owners, never a placeholder empty board.
    pub(in crate::app) fn initialize_snapshot_projection(&mut self) {
        if self.snapshot_observations.ready {
            return;
        }
        self.cli_readings.warm();
        self.revive_parked_projects();
        self.warm_snapshot_board_observations();
        self.refresh_workspace_snapshot_observations();
        self.warm_snapshot_board_observations();
        self.board_list();
        let rows = self.capture_project_snapshot_observations();
        let observed = rows
            .iter()
            .map(|row| (row.project_id.clone(), row.render()))
            .collect();
        self.install_project_snapshot_observations(observed);
        self.refresh_snapshot_observations();
        self.refresh_snapshot_task_observations().expect(
            "initial watched-task observations must be authoritative before snapshot admission",
        );
        let histories = self
            .prepare_snapshot_histories(SnapshotDelta::new(self.read_view.next_version()), None);
        self.read_view.publish_delta(histories);
        self.snapshot_observations.ready = true;
        self.publish_all_snapshot_projection();
    }

    fn snapshot_owner_ids(&self) -> Vec<String> {
        self.plans.keys().chain(self.runs.keys()).cloned().collect()
    }

    pub(in crate::app) fn publish_snapshot_projection(&self) {
        if !self.can_publish_snapshot_projection() {
            return;
        }
        let previous = self.read_view.load();
        let delta = self
            .prepare_global_snapshot(SnapshotDelta::new(self.read_view.next_version()), &previous);
        let delta = self.prepare_workspace_snapshot_details(delta, &previous, None);
        let owners = self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clone();
        let delta = self.prepare_snapshot_owner_rows(delta, &previous, &owners, false);
        self.publish_prepared_snapshot_projection(delta);
    }

    pub(in crate::app) fn publish_all_snapshot_projection(&self) {
        if let Some(delta) = self.prepare_all_snapshot_projection() {
            self.publish_prepared_snapshot_projection(delta);
        }
    }

    pub(in crate::app) fn prepare_all_snapshot_projection(&self) -> Option<SnapshotDelta> {
        if !self.snapshot_observations.ready {
            return None;
        }
        let pending_histories = self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clone();
        self.snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .extend(self.snapshot_owner_ids());
        if !self.can_publish_snapshot_projection() {
            return None;
        }
        let previous = self.read_view.load();
        let delta = self
            .prepare_global_snapshot(SnapshotDelta::new(self.read_view.next_version()), &previous);
        let delta = self.prepare_workspace_snapshot_details(delta, &previous, None);
        let owners = self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clone();
        let mut delta = self.prepare_snapshot_agent_rows(delta, &previous, &owners);
        for owner in pending_histories {
            delta = self.prepare_snapshot_histories(delta, Some(&owner));
        }
        Some(delta)
    }

    pub(in crate::app) fn flush_pending_snapshot_projection(&self) {
        if self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .is_empty()
        {
            return;
        }
        self.publish_snapshot_projection();
    }

    /// Include this owner's agent rows in the next atomic metadata publication.
    pub(in crate::app) fn mark_snapshot_owner_pending(&self, owner: &str) {
        if self.snapshot_observations.ready {
            self.snapshot_observations
                .pending_owners
                .lock()
                .unwrap()
                .insert(owner.to_owned());
        }
    }

    pub(in crate::app) fn publish_owner_snapshot_projection(&self, owner: &str) {
        if let Some(delta) = self.prepare_owner_snapshot_projection(owner) {
            self.publish_prepared_snapshot_projection(delta);
        }
    }

    pub(in crate::app) fn prepare_owner_snapshot_projection(
        &self,
        owner: &str,
    ) -> Option<SnapshotDelta> {
        if !self.snapshot_observations.ready {
            return None;
        }
        self.snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .insert(owner.to_owned());
        if !self.can_publish_snapshot_projection() {
            return None;
        }
        let previous = self.read_view.load();
        let delta = self
            .prepare_global_snapshot(SnapshotDelta::new(self.read_view.next_version()), &previous);
        let owners = self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clone();
        Some(self.prepare_snapshot_owner_rows(delta, &previous, &owners, true))
    }

    pub(in crate::app) fn publish_prepared_snapshot_projection(&self, delta: SnapshotDelta) {
        self.read_view.publish_delta(delta);
        self.snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clear();
    }

    fn can_publish_snapshot_projection(&self) -> bool {
        self.snapshot_observations.ready && !self.committed_entities.has_borrowed(self)
    }

    fn prepare_snapshot_owner_rows(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owners: &BTreeSet<String>,
        workspace_details: bool,
    ) -> SnapshotDelta {
        delta = self.prepare_snapshot_agent_rows(delta, previous, owners);
        for owner in owners {
            if workspace_details {
                delta = self.prepare_workspace_snapshot_details(delta, previous, Some(owner));
            }
            delta = self.prepare_snapshot_histories(delta, Some(owner));
        }
        delta
    }

    fn prepare_snapshot_agent_rows(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owners: &BTreeSet<String>,
    ) -> SnapshotDelta {
        for owner in owners {
            delta = match self
                .agent_list(&json!({"entity_id": owner, "include_project_workers": true}))
            {
                Ok(value) => put_changed(delta, previous, "agent.list", owner, value),
                Err(_) => delta.remove("agent.list", owner),
            };
        }
        delta
    }

    pub(in crate::app) fn prepare_global_snapshot(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        let board = self.board_snapshot();
        let projects = self.snapshot_project_list();
        let workspaces = self
            .workspace_list_snapshot(&json!({}))
            .unwrap_or_else(|error| {
                eprintln!("workspace snapshot: {error}");
                previous
                    .get("workspace.list", "")
                    .cloned()
                    .expect("workspace observations warmed before publication")
            });
        let lists = json!({"projects": projects["projects"], "workspaces": snapshot_push_workspace_rows(&workspaces["workspaces"], &board["workspace_summaries"]), "usage_limits": board["usage_limits"]});
        delta = put_changed(delta, previous, "board.list", "", board);
        delta = put_changed(delta, previous, "project.list", "", projects);
        delta = put_changed(delta, previous, "workspace.list", "", workspaces.clone());
        delta = self.prepare_workspace_lists(delta, previous, &workspaces);
        delta = put_changed(
            delta,
            previous,
            "settings.get",
            "",
            self.settings_get_observed(self.snapshot_account_availability()),
        );
        delta = put_changed(
            delta,
            previous,
            "models.list",
            "",
            self.models_list_snapshot(),
        );
        delta.set_facts(Arc::new(self.push_facts_snapshot(lists)))
    }

    fn prepare_workspace_lists(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        workspaces: &Value,
    ) -> SnapshotDelta {
        let projects = self
            .projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<BTreeSet<_>>();
        for key in previous
            .keys()
            .filter(|key| key.method == "workspace.list" && !key.owner.is_empty())
        {
            if !projects.contains(key.owner.as_str()) {
                delta = delta.remove(key.method, key.owner.clone());
            }
        }
        for project in projects {
            let rows = workspaces["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|workspace| workspace["project_id"] == project)
                .cloned()
                .collect::<Vec<_>>();
            delta = put_changed(
                delta,
                previous,
                "workspace.list",
                project,
                json!({"workspaces": rows}),
            );
        }
        delta
    }
}

fn snapshot_push_workspace_rows(workspaces: &Value, summaries: &Value) -> Value {
    let rows = workspaces
        .as_array()
        .into_iter()
        .flatten()
        .map(|workspace| {
            let mut workspace = workspace.clone();
            if let Some(summary) = summaries
                .as_array()
                .into_iter()
                .flatten()
                .find(|summary| summary["workspace_id"] == workspace["workspace_id"])
            {
                for key in ["work_summary", "can_finish", "finish_blockers"] {
                    workspace[key] = summary[key].clone();
                }
            }
            workspace
        })
        .collect::<Vec<_>>();
    json!(rows)
}

fn put_changed(
    delta: SnapshotDelta,
    previous: &ReadView,
    method: &'static str,
    owner: &str,
    value: Value,
) -> SnapshotDelta {
    if previous.get(method, owner) == Some(&value) {
        delta
    } else {
        delta.put(method, owner, value)
    }
}

mod history;
mod maintenance;
mod observations;
mod projects;

#[cfg(test)]
mod tests;
