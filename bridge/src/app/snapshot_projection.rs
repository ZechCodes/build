//! Producer-owned observations and immutable wire projections. Expensive probes
//! are captured by lifecycle work; publication consumes only owned observations.

use super::read_view::{ReadView, SnapshotDelta};
use super::workspaces::ProjectionOwnership;
use super::{AppState, ProjectListRow};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(in crate::app) struct SnapshotObservations {
    ready: bool,
    pending_owners: Mutex<BTreeSet<String>>,
    ownership: Mutex<Option<Arc<ProjectionOwnership>>>,
    paths: BTreeMap<PathBuf, PathBuf>,
    existing_paths: BTreeSet<PathBuf>,
    branches: BTreeMap<String, String>,
    watched_tasks: Mutex<BTreeMap<String, Vec<Arc<Value>>>>,
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
        let _ownership = self.begin_snapshot_ownership();
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

    /// A committed task observation changes the watched-task rows. Keep the
    /// usual publication barrier and fall back when other owners are pending.
    pub(in crate::app) fn publish_task_snapshot_projection(&self) {
        if !self.can_publish_snapshot_projection() {
            return;
        }
        if !self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .is_empty()
        {
            self.publish_snapshot_projection();
            return;
        }
        let _ownership = self.begin_snapshot_ownership();
        let previous = self.read_view.load();
        if let Some(delta) = self.prepare_incremental_task_snapshot(
            SnapshotDelta::new(self.read_view.next_version()),
            &previous,
        ) {
            self.publish_prepared_snapshot_projection(delta);
        } else {
            self.publish_snapshot_projection();
        }
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
        let _ownership = self.begin_snapshot_ownership();
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
            .extend(self.snapshot_affected_owner_ids(owner));
        if !self.can_publish_snapshot_projection() {
            return None;
        }
        let _ownership = self.begin_snapshot_ownership();
        let previous = self.read_view.load();
        let owners = self
            .snapshot_observations
            .pending_owners
            .lock()
            .unwrap()
            .clone();
        let delta = self
            .prepare_incremental_owner_snapshot(
                SnapshotDelta::new(self.read_view.next_version()),
                &previous,
                &owners,
            )
            .unwrap_or_else(|| {
                self.prepare_global_snapshot(
                    SnapshotDelta::new(self.read_view.next_version()),
                    &previous,
                )
            });
        Some(self.prepare_snapshot_owner_rows(delta, &previous, &owners, true))
    }

    pub(in crate::app) fn publish_prepared_snapshot_projection(&self, delta: SnapshotDelta) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.install");
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
        delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.global");
        self.prepare_full_global_snapshot(delta, previous)
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
                Value::Object(serde_json::Map::from_iter([(
                    "workspaces".into(),
                    Value::Array(rows),
                )])),
            );
        }
        delta
    }
}

fn snapshot_push_workspace_rows(workspaces: &Value, summaries: &Value) -> Value {
    let mut by_workspace = HashMap::new();
    for summary in summaries.as_array().into_iter().flatten() {
        #[cfg(test)]
        summary_counts::join_visit();
        // The old search selected the first equal id, including missing/null
        // and legacy non-string ids. Keep Value equality for every input.
        by_workspace
            .entry(&summary["workspace_id"])
            .or_insert(summary);
    }
    let rows = workspaces
        .as_array()
        .into_iter()
        .flatten()
        .map(|workspace| {
            let mut workspace = workspace.clone();
            if let Some(summary) = by_workspace.get(&workspace["workspace_id"]) {
                for key in ["work_summary", "can_finish", "finish_blockers"] {
                    workspace[key] = summary[key].clone();
                }
            }
            workspace
        })
        .collect::<Vec<_>>();
    Value::Array(rows)
}

fn put_changed(
    delta: SnapshotDelta,
    previous: &ReadView,
    method: &'static str,
    owner: &str,
    value: Value,
) -> SnapshotDelta {
    if previous
        .get_shared(method, owner)
        .is_some_and(|previous| previous.equals_value(&value))
    {
        delta
    } else {
        delta.put(method, owner, value)
    }
}

mod history;
pub(in crate::app) mod incremental;
mod maintenance;
mod observations;
mod ownership;
mod projects;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(in crate::app) mod work_counts;

#[cfg(test)]
pub(in crate::app) mod summary_counts;
