//! Sparse producer inputs; only bootstrap and membership changes enumerate IDs.
use super::*;
use std::collections::BTreeSet;

#[cfg(test)]
mod workspace_dependents_tests;

#[derive(Default)]
pub(in crate::app::observation_reads) struct ScopePatch {
    runs: BTreeMap<String, Option<Arc<RunInput>>>,
    plan_projects: BTreeMap<String, Option<String>>,
    projects: BTreeMap<String, Option<Arc<ProjectInput>>>,
    workspaces: BTreeMap<String, Option<Arc<Workspace>>>,
    order: Option<Vec<String>>,
    workspace_roots: BTreeMap<String, Option<PathBuf>>,
}

fn project(app: &AppState, id: &str) -> Option<Arc<ProjectInput>> {
    #[cfg(test)]
    super::super::capture_metrics::record("scope_project", id);
    let project = app.projects.get(id)?;
    let scan = app.board.diff().external_scan(id);
    Some(Arc::new(ProjectInput {
        path: project.repo_path.clone(),
        is_git: project.is_git,
        base_branch: project.base_branch.clone(),
        sources: project.sources.clone(),
        external: scan.worktrees.to_vec(),
        scan_settled: scan.settled_at.is_some(),
    }))
}

fn run(app: &AppState, id: &str) -> Option<Arc<RunInput>> {
    #[cfg(test)]
    super::super::capture_metrics::record("scope_run", id);
    let run = app.runs.get(id)?;
    let checkout_git = !app.snapshot_projection_initialized()
        || app.snapshot_path_exists(&run.worktree.path.join(".git"));
    let workspace_id = app
        .snapshot_projection_initialized()
        .then(|| {
            app.workspace_snapshot_for_conversation(id, None)
                .map(|workspace| workspace.id.clone())
        })
        .flatten();
    Some(Arc::new(RunInput {
        plan_id: run.run.plan_id.as_ref().map(|id| id.0.clone()),
        created_at: app
            .board
            .attention()
            .clock(id)
            .created_at
            .unwrap_or_default(),
        workspace_id,
        checkout_git,
        canonical_checkout: if app.snapshot_projection_initialized() {
            app.snapshot_root(&run.worktree.path)
        } else {
            run.worktree.path.clone()
        },
        project_id: app.projects.project_id_of(id).map(str::to_owned),
        project_path: PathBuf::from(app.project_path_for(id)),
        checkout: run.worktree.path.clone(),
        git_root: run.worktree.path.clone(),
        base_sha: run.base_sha.clone(),
        base_branch: run.worktree.base_branch.clone(),
        stages: run.stages.clone(),
        terminal: run.run.state.is_terminal(),
    }))
}

impl ScopePatch {
    pub fn owner(&mut self, app: &AppState, owner: &str) {
        self.runs.insert(owner.into(), run(app, owner));
        self.plan_project(app, owner);
    }
    fn plan_project(&mut self, app: &AppState, owner: &str) {
        self.plan_projects.insert(
            owner.into(),
            app.plans
                .contains_key(owner)
                .then(|| app.projects.project_id_of(owner).map(str::to_owned))
                .flatten(),
        );
    }
    pub fn project(&mut self, app: &AppState, id: &str) {
        self.projects.insert(id.into(), project(app, id));
    }
    pub fn workspace(&mut self, app: &AppState, id: &str) {
        #[cfg(test)]
        super::super::capture_metrics::record("scope_workspace", id);
        self.workspaces
            .insert(id.into(), app.workspaces.get(id).cloned().map(Arc::new));
        self.workspace_roots.insert(
            id.into(),
            app.workspaces
                .get(id)
                .map(|workspace| app.snapshot_root(&workspace.root)),
        );
    }
    pub fn topology(&mut self, app: &AppState) {
        self.order = Some(
            app.runs
                .keys()
                .inspect(|_id| {
                    #[cfg(test)]
                    super::super::capture_metrics::record("scope_topology", _id);
                })
                .cloned()
                .collect(),
        );
    }
}

impl ScopeReadContext {
    pub fn capture(app: &AppState) -> Self {
        let mut patch = ScopePatch::default();
        for id in app.projects.iter().map(|project| &project.id) {
            patch.project(app, id);
        }
        for workspace in app.workspaces.list(None) {
            patch.workspace(app, &workspace.id);
        }
        for id in app.runs.keys() {
            patch.owner(app, id);
        }
        for owner in app.plans.keys() {
            patch.plan_project(app, owner);
        }
        patch.topology(app);
        let mut context = Self::default();
        context.apply(&patch);
        context
    }

    pub(in crate::app) fn physical_changed(&self, app: &AppState, id: &str) -> bool {
        self.runs.get(id).is_some_and(|run| {
            run.checkout_git != app.snapshot_path_exists(&run.checkout.join(".git"))
                || run.canonical_checkout != app.snapshot_root(&run.checkout)
        })
    }

    pub(in crate::app) fn workspace_physical_changed(&self, app: &AppState, id: &str) -> bool {
        self.workspace_canonical.get(id)
            != app
                .workspaces
                .get(id)
                .map(|workspace| app.snapshot_root(&workspace.root))
                .as_ref()
    }

    // Lifecycle changes can displace a different owner at the same root. The
    // root index bounds this closure; ordinary status samples skip it entirely.
    pub(in crate::app::observation_reads) fn ownership_dependents(
        &self,
        app: &AppState,
        id: &str,
    ) -> Vec<String> {
        let old = self.runs.get(id);
        let current = app.runs.get(id);
        let current_workspace = current
            .and_then(|_| app.workspace_snapshot_for_conversation(id, None))
            .map(|workspace| workspace.id.clone());
        let unchanged = match (old, current) {
            (Some(old), Some(current)) => {
                old.terminal == current.run.state.is_terminal()
                    && old.checkout == current.worktree.path
                    && old.canonical_checkout == app.snapshot_root(&current.worktree.path)
                    && old.project_id.as_deref() == app.projects.project_id_of(id)
                    && old.workspace_id == current_workspace
            }
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return Vec::new();
        }
        let mut workspaces = BTreeSet::new();
        if let Some(workspace) = old.and_then(|old| old.workspace_id.as_ref()) {
            workspaces.insert(workspace.clone());
        }
        if let Some(workspace) = current_workspace {
            workspaces.insert(workspace);
        }
        if app.workspaces.get(id).is_some() {
            workspaces.insert(id.to_owned());
        }
        let roots = old
            .map(|old| old.canonical_checkout.clone())
            .into_iter()
            .chain(current.map(|current| app.snapshot_root(&current.worktree.path)));
        for root in roots {
            if let Some(ids) = self.workspace_roots.get(&root) {
                workspaces.extend(ids.iter().cloned());
            }
        }
        let mut owners = BTreeSet::new();
        for workspace in workspaces {
            owners.extend(self.workspace_dependents(&workspace));
            if let Some(owner) = app
                .workspaces
                .get(&workspace)
                .and_then(|workspace| app.workspace_snapshot_conversation_owner(workspace))
            {
                owners.insert(owner);
            }
        }
        owners.into_iter().collect()
    }

    pub(in crate::app::observation_reads) fn workspace_dependents(&self, id: &str) -> Vec<String> {
        self.workspace_dependents
            .get(id)
            .map(|owners| owners.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(in crate::app::observation_reads) fn contains_project(&self, id: &str) -> bool {
        self.projects.contains_key(id)
    }

    pub(in crate::app::observation_reads) fn contains_run(&self, id: &str) -> bool {
        self.runs.contains_key(id)
    }

    pub(in crate::app::observation_reads) fn apply(&mut self, patch: &ScopePatch) {
        for (id, row) in &patch.projects {
            replace(&mut self.projects, id, row);
        }
        for (owner, project) in &patch.plan_projects {
            match project {
                Some(id) => {
                    self.plan_projects.insert(owner.clone(), id.clone());
                }
                None => {
                    self.plan_projects.remove(owner);
                }
            }
        }
        for (id, row) in &patch.workspaces {
            if let Some(root) = self.workspace_canonical.remove(id) {
                if let Some(mut ids) = self.workspace_roots.get(&root).cloned() {
                    ids.remove(id);
                    if ids.is_empty() {
                        self.workspace_roots.remove(&root);
                    } else {
                        self.workspace_roots.insert(root, ids);
                    }
                }
            }
            if let Some(Some(root)) = patch.workspace_roots.get(id) {
                let mut ids = self.workspace_roots.get(root).cloned().unwrap_or_default();
                ids.insert(id.clone());
                self.workspace_roots.insert(root.clone(), ids);
                self.workspace_canonical.insert(id.clone(), root.clone());
            }
            replace(&mut self.workspaces, id, row);
        }
        let mut changed = BTreeSet::new();
        for (id, row) in &patch.runs {
            if let Some(workspace) = self
                .runs
                .get(id)
                .and_then(|run| run.workspace_id.as_ref())
                .cloned()
            {
                let mut owners = self
                    .workspace_dependents
                    .get(&workspace)
                    .cloned()
                    .unwrap_or_default();
                #[cfg(test)]
                super::super::capture_metrics::record("scope_membership_remove", id);
                owners.remove(id);
                self.workspace_dependents.insert(workspace, owners);
            }
            if let Some(workspace) = row.as_ref().and_then(|run| run.workspace_id.as_ref()) {
                let mut owners = self
                    .workspace_dependents
                    .get(workspace)
                    .cloned()
                    .unwrap_or_default();
                #[cfg(test)]
                super::super::capture_metrics::record("scope_membership_insert", id);
                owners.insert(id.clone());
                self.workspace_dependents.insert(workspace.clone(), owners);
            }
            let before = self
                .runs
                .get(id)
                .map(|run| (run.plan_id.clone(), run.created_at.clone()));
            let after = row
                .as_ref()
                .map(|run| (run.plan_id.clone(), run.created_at.clone()));
            if before != after {
                if let Some(Some(plan)) = before.map(|value| value.0) {
                    self.remove_lineage(&plan, id);
                    changed.insert(plan);
                }
                if let Some(Some(plan)) = after.map(|value| value.0) {
                    self.add_lineage(&plan, id);
                    changed.insert(plan);
                }
            }
            replace(&mut self.runs, id, row);
        }
        if let Some(order) = &patch.order {
            self.order = Arc::new(
                order
                    .iter()
                    .enumerate()
                    .map(|(rank, id)| (id.clone(), rank))
                    .collect(),
            );
            changed.extend(self.lineages.keys().cloned());
        }
        for plan in changed {
            #[cfg(test)]
            super::super::capture_metrics::record("scope_lineage", &plan);
            self.sort_lineage(&plan);
        }
    }

    fn remove_lineage(&mut self, plan: &str, id: &str) {
        let mut ids = self
            .lineages
            .get(plan)
            .map(|ids| ids.as_ref().clone())
            .unwrap_or_default();
        ids.retain(|candidate| candidate != id);
        self.lineages.insert(plan.into(), Arc::new(ids));
    }
    fn add_lineage(&mut self, plan: &str, id: &str) {
        let mut ids = self
            .lineages
            .get(plan)
            .map(|ids| ids.as_ref().clone())
            .unwrap_or_default();
        ids.push(id.into());
        self.lineages.insert(plan.into(), Arc::new(ids));
    }
    fn sort_lineage(&mut self, plan: &str) {
        let mut ids = self
            .lineages
            .get(plan)
            .map(|ids| ids.as_ref().clone())
            .unwrap_or_default();
        ids.sort_by_key(|id| {
            (
                &self.runs[id].created_at,
                self.order.get(id).copied().unwrap_or_default(),
            )
        });
        self.lineages.insert(plan.into(), Arc::new(ids));
    }
}

fn replace<T>(map: &mut OrdMap<String, Arc<T>>, id: &str, row: &Option<Arc<T>>) {
    match row {
        Some(row) => {
            map.insert(id.into(), row.clone());
        }
        None => {
            map.remove(id);
        }
    }
}
