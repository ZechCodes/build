//! Ownership shared by every row in one immutable projection. This context
//! lives only while its candidate is rendered; it never survives a mutation.
use super::{AppState, Workspace};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(in crate::app) struct ProjectionOwnership {
    pub(in crate::app) rendering: super::projection_rendering::ProjectionRendering,
    project_owners: BTreeSet<String>,
    workspace_owners: BTreeMap<String, Option<String>>,
    owner_workspaces: BTreeMap<String, String>,
    owner_project_workspaces: BTreeMap<(String, String), String>,
    surviving_workspaces: BTreeMap<String, String>,
    compaction_workspaces: BTreeMap<String, String>,
}

impl ProjectionOwnership {
    pub(in crate::app) fn is_project_owner(&self, owner: &str) -> bool {
        self.project_owners.contains(owner)
    }

    pub(in crate::app) fn workspace_owner(&self, workspace: &str) -> Option<Option<&String>> {
        self.workspace_owners.get(workspace).map(Option::as_ref)
    }

    pub(in crate::app) fn workspace_for_owner(
        &self,
        owner: &str,
        project: Option<&str>,
    ) -> Option<&str> {
        match project {
            Some(project) => self
                .owner_project_workspaces
                .get(&(owner.into(), project.into())),
            None => self.owner_workspaces.get(owner),
        }
        .map(String::as_str)
    }

    pub(in crate::app) fn surviving_workspace(&self, owner: &str) -> Option<&str> {
        self.surviving_workspaces.get(owner).map(String::as_str)
    }

    pub(in crate::app) fn compaction_workspace(&self, owner: &str) -> Option<&str> {
        self.compaction_workspaces.get(owner).map(String::as_str)
    }

    fn capture_workspace(&mut self, app: &AppState, workspace: &Workspace) {
        let owner = self.choose_workspace_owner(app, workspace);
        if let Some(owner) = &owner {
            // Registry order is the existing name-ordered reverse-lookup
            // precedence, separately within each optional project filter.
            self.owner_workspaces
                .entry(owner.clone())
                .or_insert_with(|| workspace.id.clone());
            self.owner_project_workspaces
                .entry((owner.clone(), workspace.project_id.clone()))
                .or_insert_with(|| workspace.id.clone());
        }
        self.workspace_owners.insert(workspace.id.clone(), owner);
    }

    fn choose_workspace_owner(&self, app: &AppState, workspace: &Workspace) -> Option<String> {
        if app.runs.contains_key(&workspace.id) && !self.is_project_owner(&workspace.id) {
            return Some(workspace.id.clone());
        }
        app.runs
            .iter()
            .find(|(owner, active)| {
                !active.run.state.is_terminal()
                    && !self.is_project_owner(owner)
                    && app.projects.project_id_of(owner) == Some(workspace.project_id.as_str())
                    && app.observed_same_path(&active.worktree.path, &workspace.root)
            })
            .map(|(owner, _)| owner.clone())
    }

    fn capture_run_scopes(&mut self, app: &AppState, owner: &str, workspaces: &[&Workspace]) {
        let active = &app.runs[owner];
        if let Some(project) = app.projects.project_id_of(owner) {
            if let Some(workspace) = workspaces.iter().find(|workspace| {
                workspace.project_id == project
                    && (workspace.id == owner
                        || app.observed_same_path(&workspace.root, &active.worktree.path))
            }) {
                self.surviving_workspaces
                    .insert(owner.into(), workspace.id.clone());
            }
        }
        if self.is_project_owner(owner) {
            return;
        }
        if let Some(workspace) = self.owner_workspaces.get(owner) {
            self.compaction_workspaces
                .insert(owner.into(), workspace.clone());
            return;
        }
        self.capture_parked_compaction_scope(app, owner, workspaces);
    }

    fn capture_parked_compaction_scope(
        &mut self,
        app: &AppState,
        owner: &str,
        workspaces: &[&Workspace],
    ) {
        let active = &app.runs[owner];
        if app.projects.project_id_of(owner).is_some() || active.run.state.is_terminal() {
            return;
        }
        let Some(project) = app.compaction_project_of(owner) else {
            return;
        };
        if let Some(workspace) = workspaces.iter().find(|workspace| {
            workspace.project_id == project
                && app.observed_same_path(&workspace.root, &active.worktree.path)
        }) {
            self.compaction_workspaces
                .insert(owner.into(), workspace.id.clone());
        }
    }
}

impl AppState {
    pub(in crate::app) fn capture_snapshot_ownership(&self) -> ProjectionOwnership {
        let mut context = ProjectionOwnership {
            rendering: super::projection_rendering::ProjectionRendering::capture(self),
            project_owners: self
                .runs
                .keys()
                .filter(|owner| self.is_project_snapshot_conversation_owner(owner))
                .cloned()
                .collect(),
            ..ProjectionOwnership::default()
        };
        let workspaces = self.workspaces.list(None);
        for workspace in &workspaces {
            context.capture_workspace(self, workspace);
        }
        for owner in self.runs.keys() {
            context.capture_run_scopes(self, owner, &workspaces);
        }
        context
    }
}
