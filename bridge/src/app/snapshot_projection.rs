//! Producer-owned observations and immutable wire projections. Expensive probes
//! are captured by lifecycle work; publication consumes only owned observations.

use super::read_view::{ReadView, SnapshotDelta};
use super::{AppState, ProjectListRow};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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

struct ProjectObservation {
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
        let histories = self
            .prepare_snapshot_histories(SnapshotDelta::new(self.read_view.next_version()), None);
        self.read_view.publish_delta(histories);
        self.snapshot_observations.ready = true;
        self.publish_all_snapshot_projection();
    }

    fn warm_snapshot_board_observations(&mut self) {
        let run_ids = self.runs.keys().cloned().collect::<Vec<_>>();
        let project_ids = self
            .projects
            .iter()
            .filter(|project| project.is_git)
            .map(|project| project.id.clone())
            .collect::<Vec<_>>();
        let mut jobs = run_ids
            .iter()
            .filter(|id| self.board.diff().run_stat(id).is_none())
            .filter_map(|id| self.run_stat_refresh(id))
            .collect::<Vec<_>>();
        jobs.extend(
            project_ids
                .iter()
                .filter(|id| self.board.diff().external_scan(id).settled_at.is_none())
                .filter_map(|id| self.external_scan_refresh(id)),
        );
        let memberships = self
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| {
                (
                    workspace.id.clone(),
                    workspace
                        .directories
                        .iter()
                        .filter(|directory| directory.is_git)
                        .map(|directory| directory.path.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        self.sync_workspace_summaries(&memberships);
        jobs.extend(
            memberships
                .into_iter()
                .filter(|(id, repositories)| self.workspace_summary_of(id, repositories).is_none())
                .map(|(id, repositories)| self.workspace_summary_refresh(&id, repositories)),
        );
        for job in jobs {
            if let Some(entry) = job.compute(None) {
                self.store_diff_entry(entry);
            }
        }
    }

    pub(in crate::app) fn capture_project_snapshot_observations(&self) -> Vec<ProjectListRow> {
        self.project_list_rows()
    }

    /// Call after rendering the captured rows off the app lock. The caller
    /// keeps the lifecycle generation fence through this installation.
    pub(in crate::app) fn install_project_snapshot_observations(
        &mut self,
        rows: Vec<(String, Value)>,
    ) {
        let version = self.read_view.next_version();
        *self.snapshot_observations.projects.lock().unwrap() = rows
            .into_iter()
            .map(|(id, value)| (id, ProjectObservation { version, value }))
            .collect();
    }

    pub(in crate::app) fn capture_project_snapshot_refresh(&self) -> ProjectSnapshotRefresh {
        ProjectSnapshotRefresh {
            version: self.read_view.next_version(),
            rows: self.project_list_rows(),
        }
    }

    pub(in crate::app) fn install_project_snapshot_refresh(
        &self,
        refresh: RenderedProjectSnapshotRefresh,
    ) {
        let current = self.project_list_rows();
        let mut observations = self.snapshot_observations.projects.lock().unwrap();
        for (row, value) in refresh.rows {
            let unchanged = current.iter().any(|current| {
                current.project_id == row.project_id
                    && current.repo_path == row.repo_path
                    && current.worktrees_root == row.worktrees_root
                    && current.sources == row.sources
            });
            let newer = observations
                .get(&row.project_id)
                .is_none_or(|old| old.version < refresh.version);
            if unchanged && newer {
                observations.insert(
                    row.project_id,
                    ProjectObservation {
                        version: refresh.version,
                        value,
                    },
                );
            }
        }
        observations.retain(|id, _| current.iter().any(|row| row.project_id == *id));
    }

    pub(in crate::app) fn has_current_project_snapshot_observation(
        &self,
        project: &super::projects::Project,
    ) -> bool {
        let observations = self.snapshot_observations.projects.lock().unwrap();
        let Some(observed) = observations.get(&project.id) else {
            return false;
        };
        let Some(sources) = observed.value["sources"].as_array() else {
            return false;
        };
        observed.value["path"] == project.repo_path.display().to_string()
            && sources.len() == project.sources.len()
            && project.sources.iter().all(|source| {
                sources.iter().any(|row| {
                    row["id"] == source.id
                        && row["path"] == source.path.display().to_string()
                        && row["is_git"] == source.is_git
                })
            })
    }

    pub(in crate::app) fn record_project_snapshot_observation(&self, project: &str, value: &Value) {
        self.snapshot_observations.projects.lock().unwrap().insert(
            project.to_owned(),
            ProjectObservation {
                version: self.read_view.next_version(),
                value: value.clone(),
            },
        );
    }

    fn snapshot_account_availability(&self) -> Value {
        self.projects
            .iter()
            .next()
            .and_then(|project| {
                self.snapshot_observations
                    .projects
                    .lock()
                    .unwrap()
                    .get(&project.id)
                    .map(|observed| observed.value["isolation_available"].clone())
            })
            .unwrap_or_else(|| {
                json!(crate::isolation::IsolationAvailability::unavailable(
                    "no project registered yet"
                ))
            })
    }

    /// Lifecycle/background producer work. This is never called by a snapshot
    /// read. Project repository/volume probes have a separate off-lock phase.
    pub(in crate::app) fn refresh_snapshot_observations(&mut self) {
        let mut paths = self.snapshot_paths();
        paths.extend(
            paths
                .iter()
                .map(|path| path.join(".git"))
                .collect::<Vec<_>>(),
        );
        self.snapshot_observations.existing_paths =
            paths.iter().filter(|path| path.exists()).cloned().collect();
        self.snapshot_observations.branches = self
            .runs
            .iter()
            .map(|(id, run)| (id.clone(), run.worktree.branch()))
            .collect();
        self.snapshot_observations.paths = paths
            .into_iter()
            .map(|path| {
                let canonical = Self::canonical_root(&path);
                (path, canonical)
            })
            .collect();
        let workspaces = self
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| workspace.id.clone())
            .collect::<Vec<_>>();
        self.snapshot_observations
            .workspace_reviews
            .retain(|id, _| workspaces.contains(id));
        for id in workspaces {
            self.refresh_workspace_review_observation(&id);
        }
    }

    pub(in crate::app) fn refresh_workspace_snapshot_observation(&mut self, id: &str) {
        let Some(workspace) = self.workspaces.get(id).cloned() else {
            self.snapshot_observations.workspace_reviews.remove(id);
            return;
        };
        let paths = std::iter::once(workspace.root.clone())
            .chain(
                workspace
                    .directories
                    .iter()
                    .map(|directory| directory.path.clone()),
            )
            .collect::<Vec<_>>();
        for path in paths {
            self.observe_snapshot_path(path);
        }
        self.refresh_workspace_review_observation(id);
    }

    fn observe_snapshot_path(&mut self, path: PathBuf) {
        let canonical = Self::canonical_root(&path);
        for observed in [path.clone(), path.join(".git")] {
            if observed.exists() {
                self.snapshot_observations.existing_paths.insert(observed);
            } else {
                self.snapshot_observations.existing_paths.remove(&observed);
            }
        }
        self.snapshot_observations.paths.insert(path, canonical);
    }

    pub(in crate::app) fn refresh_owner_snapshot_observations(&mut self, owner: &str) {
        if let Some(active) = self.runs.get(owner) {
            self.snapshot_observations
                .branches
                .insert(owner.to_owned(), active.worktree.branch());
        }
        let path = self
            .plans
            .get(owner)
            .and_then(|plan| plan.workspace.as_ref())
            .map(|workspace| workspace.checkout.clone())
            .or_else(|| self.runs.get(owner).map(|run| run.worktree.path.clone()));
        if let Some(path) = path {
            self.observe_snapshot_path(path);
        }
        if self.runs.get(owner).is_some_and(|run| run.pinned_agent_id.is_some()) {
            if let Some(scratch) = self.project_snapshot_scratch_path(owner) { self.observe_snapshot_path(scratch); }
        }
    }

    fn prepare_snapshot_histories(
        &self,
        mut delta: SnapshotDelta,
        owner: Option<&str>,
    ) -> SnapshotDelta {
        let previous = self.read_view.load();
        let owners = owner
            .map(|owner| vec![owner.to_owned()])
            .unwrap_or_else(|| self.snapshot_owner_ids());
        let histories = owners
            .into_iter()
            .flat_map(|owner| self.owner_snapshot_histories(&previous, &owner))
            .collect::<BTreeMap<_, _>>();
        let retained = histories.keys().cloned().collect::<BTreeSet<_>>();
        for (id, history) in histories {
            match history {
                Ok(history) => delta = delta.put_history(id, history),
                Err(error) => eprintln!("snapshot history warm {id}: {error}"),
            }
        }
        self.remove_snapshot_histories(delta, &previous, owner, &retained)
    }

    fn owner_snapshot_histories(
        &self,
        previous: &ReadView,
        owner: &str,
    ) -> Vec<(
        String,
        Result<super::read_view::history::SharedHistory, String>,
    )> {
        let Ok(agents) = self.entity_agents(owner) else {
            return Vec::new();
        };
        agents
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(owner, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let id = thread.agent.id.clone();
                let history = match previous.history(&id) {
                    Some(history) => Ok(history.updated(thread)),
                    None => super::read_view::history::SharedHistory::warm(
                        thread,
                        (!self.snapshot_observations.ready)
                            .then_some(self.store.as_ref())
                            .flatten(),
                    ),
                };
                (id, history)
            })
            .collect()
    }

    fn remove_snapshot_histories(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owner: Option<&str>,
        retained: &BTreeSet<String>,
    ) -> SnapshotDelta {
        let removed = match owner {
            None => previous
                .history_keys()
                .filter(|id| !retained.contains(*id))
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Some(owner) => previous
                .get("agent.list", owner)
                .and_then(|row| row["agents"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|agent| agent["conversation_id"].as_str())
                .filter(|id| !retained.contains(*id) && !self.snapshot_has_conversation(id))
                .map(str::to_owned)
                .collect(),
        };
        for id in removed {
            delta = delta.remove_history(id);
        }
        delta
    }

    fn snapshot_has_conversation(&self, conversation: &str) -> bool {
        self.plans
            .values()
            .map(|plan| &plan.agents)
            .chain(self.runs.values().map(|run| &run.agents))
            .any(|roster| {
                roster
                    .iter()
                    .any(|agent| agent.conversation_id() == conversation)
            })
    }

    pub(in crate::app) fn refresh_workspace_review_observation(&mut self, id: &str) {
        let mut row = json!({});
        match self.load_workspace_review_summary(id, &mut row) {
            Ok(()) => {
                self.snapshot_observations
                    .workspace_reviews
                    .insert(id.to_owned(), row);
            }
            Err(error) => eprintln!("workspace review observation {id}: {error}"),
        }
    }

    pub(in crate::app) fn snapshot_workspace_review(&self, id: &str, row: &mut Value) {
        if let Some(summary) = self
            .snapshot_observations
            .workspace_reviews
            .get(id)
            .and_then(|row| row.get("active_review"))
        {
            row["active_review"] = summary.clone();
        }
    }

    pub(in crate::app) fn refresh_snapshot_task_observations(&self) {
        let projects = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>();
        self.snapshot_observations
            .watched_tasks
            .lock()
            .unwrap()
            .retain(|id, _| projects.contains(id));
        for id in projects {
            self.refresh_project_snapshot_task_observations(&id);
        }
    }

    pub(in crate::app) fn refresh_project_snapshot_task_observations(&self, project: &str) {
        let rows = self.watched_task_rows_for_project(project);
        self.snapshot_observations
            .watched_tasks
            .lock()
            .unwrap()
            .insert(project.to_owned(), rows);
    }

    pub(in crate::app) fn snapshot_watched_task_rows(&self) -> Vec<Value> {
        self.snapshot_observations
            .watched_tasks
            .lock()
            .unwrap()
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    pub(in crate::app) fn snapshot_run_branch(
        &self,
        owner: &str,
        worktree: &crate::worktree::Worktree,
    ) -> String {
        if !self.snapshot_observations.ready {
            return worktree.branch();
        }
        self.snapshot_observations
            .branches
            .get(owner)
            .cloned()
            .unwrap_or_else(|| worktree.recorded_branch.clone())
    }

    pub(in crate::app) fn record_snapshot_run_branch(&mut self, owner: &str, branch: String) {
        self.snapshot_observations
            .branches
            .insert(owner.to_owned(), branch);
    }

    pub(in crate::app) fn snapshot_path_exists(&self, path: &Path) -> bool {
        if self.snapshot_observations.ready {
            self.snapshot_observations.existing_paths.contains(path)
        } else {
            path.exists()
        }
    }

    pub(in crate::app) fn snapshot_root(&self, path: &Path) -> PathBuf {
        if !self.snapshot_observations.ready {
            return Self::canonical_root(path);
        }
        self.snapshot_observations
            .paths
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_owned())
    }

    pub(in crate::app) fn observed_same_path(&self, left: &Path, right: &Path) -> bool {
        if !self.snapshot_observations.ready {
            return super::workspaces::same_path(left, right);
        }
        self.snapshot_root(left) == self.snapshot_root(right)
    }

    fn snapshot_paths(&self) -> BTreeSet<PathBuf> {
        self.plans
            .values()
            .filter_map(|plan| {
                plan.workspace
                    .as_ref()
                    .map(|workspace| workspace.checkout.clone())
            })
            .chain(self.runs.values().map(|run| run.worktree.path.clone()))
            .chain(self.projects.iter().flat_map(|project| {
                std::iter::once(project.repo_path.clone())
                    .chain(project.sources.iter().map(|source| source.path.clone()))
            }))
            .chain(
                self.workspaces
                    .list(None)
                    .into_iter()
                    .flat_map(|workspace| {
                        std::iter::once(workspace.root.clone()).chain(
                            workspace
                                .directories
                                .iter()
                                .map(|directory| directory.path.clone()),
                        )
                    }),
            )
            .chain(self.project_snapshot_scratch_paths())
            .chain(
                self.session_registry
                    .agent_working_roots()
                    .into_iter()
                    .map(|(root, _)| root),
            )
            .collect()
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

    fn snapshot_project_list(&self) -> Value {
        let observed = self.snapshot_observations.projects.lock().unwrap();
        let projects = self
            .project_list_rows()
            .iter()
            .map(|row| {
                let observation = observed
                    .get(&row.project_id)
                    .expect("live project observations warmed before publication");
                row.render_observed(
                    observation.value["isolation_available"].clone(),
                    Some(&observation.value),
                )
            })
            .collect::<Vec<_>>();
        json!({"projects": projects})
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

mod maintenance;

#[cfg(test)]
mod tests;
