//! Lifecycle-owned physical, review and task observations.
use super::*;
use std::path::Path;

impl AppState {
    #[cfg(test)]
    pub(in crate::app) fn warm_snapshot_observations_for_test(&mut self) {
        self.refresh_snapshot_observations();
        self.snapshot_observations.ready = true;
    }

    pub(super) fn warm_snapshot_board_observations(&mut self) {
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
    /// Lifecycle/background producer work. This is never called by a snapshot
    /// read. Project repository/volume probes have a separate off-lock phase.
    pub(in crate::app) fn refresh_snapshot_observations(&mut self) {
        let previous_inputs = self.read_view.load();
        let previous_branches = std::mem::take(&mut self.snapshot_observations.branches);
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
        if let Some(context) = &previous_inputs.observations {
            for owner in self.runs.keys() {
                if context.scopes.physical_changed(self, owner)
                    || previous_branches.get(owner)
                        != self.snapshot_observations.branches.get(owner)
                {
                    self.mark_observation_owner_changed(owner);
                }
            }
        }
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
            if previous_inputs
                .observations
                .as_ref()
                .is_some_and(|context| context.scopes.workspace_physical_changed(self, &id))
            {
                self.workspaces.observation_changes().mark(&id);
            }
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
        self.workspaces.observation_changes().mark(id);
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
        self.mark_observation_owner_changed(owner);
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
        if self
            .runs
            .get(owner)
            .is_some_and(|run| run.pinned_agent_id.is_some())
        {
            if let Some(scratch) = self.project_snapshot_scratch_path(owner) {
                self.observe_snapshot_path(scratch);
            }
        }
    }
    pub(in crate::app) fn refresh_snapshot_task_observations(&self) -> Result<(), String> {
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
            self.refresh_project_snapshot_task_observations(&id)?;
        }
        Ok(())
    }
    pub(in crate::app) fn refresh_project_snapshot_task_observations(
        &self,
        project: &str,
    ) -> Result<(), String> {
        let rows = self.watched_task_rows_for_project(project)?;
        self.snapshot_observations
            .watched_tasks
            .lock()
            .unwrap()
            .insert(project.to_owned(), rows.into_iter().map(Arc::new).collect());
        Ok(())
    }
    pub(in crate::app) fn install_snapshot_watched_task(
        &self,
        project: &str,
        task: &str,
        row: Option<Value>,
    ) {
        let mut projects = self.snapshot_observations.watched_tasks.lock().unwrap();
        let rows = projects.entry(project.to_owned()).or_default();
        match row {
            Some(row) => match rows.iter_mut().find(|row| row["task_id"] == task) {
                Some(current) => *current = Arc::new(row),
                None => rows.push(Arc::new(row)),
            },
            None => rows.retain(|row| row["task_id"] != task),
        }
    }
    pub(in crate::app) fn snapshot_watched_task_rows(&self) -> Vec<Value> {
        self.snapshot_watched_task_shared_rows()
            .into_iter()
            .map(|row| row.as_ref().clone())
            .collect()
    }
    pub(in crate::app) fn snapshot_watched_task_shared_rows(&self) -> Vec<Arc<Value>> {
        self.snapshot_observations
            .watched_tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| self.projects.get(id).is_some())
            .flat_map(|(_, rows)| rows)
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
        self.observed_run_branch(owner, worktree)
    }

    /// Pure producer input, including callers constructing the initial context.
    pub(in crate::app) fn observed_run_branch(
        &self,
        owner: &str,
        worktree: &crate::worktree::Worktree,
    ) -> String {
        self.snapshot_observations
            .branches
            .get(owner)
            .cloned()
            .unwrap_or_else(|| worktree.recorded_branch.clone())
    }
    pub(in crate::app) fn record_snapshot_run_branch(&mut self, owner: &str, branch: String) {
        self.mark_observation_owner_changed(owner);
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
        #[cfg(test)]
        crate::workspace::read_work_counts::observed_root();
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
            return crate::app::workspaces::same_path(left, right);
        }
        #[cfg(test)]
        crate::app::workspaces::record_path_comparison();
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
}
