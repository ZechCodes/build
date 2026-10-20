//! Ownership shared by every row in one immutable projection. This context
//! lives only while its candidate is rendered; it never survives a mutation.
use super::{AppState, Workspace};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

mod inputs;

#[derive(Default)]
pub(in crate::app) struct ProjectionOwnership {
    pub(in crate::app) rendering: super::projection_rendering::ProjectionRendering,
    project_owners: BTreeSet<String>,
    workspace_owners: BTreeMap<String, Option<String>>,
    owner_workspaces: BTreeMap<String, String>,
    owner_project_workspaces: BTreeMap<(String, String), String>,
    surviving_workspaces: BTreeMap<String, String>,
    compaction_workspaces: BTreeMap<String, String>,
    task_workspaces: HashMap<String, String>,
    task_paths: HashMap<PathBuf, String>,
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

    pub(in crate::app) fn task_workspace(&self, owner: &str) -> Option<&str> {
        self.task_workspaces.get(owner).map(String::as_str)
    }

    pub(in crate::app) fn task_paths(&self) -> &HashMap<PathBuf, String> {
        &self.task_paths
    }

    fn capture_workspace(&mut self, inputs: &inputs::Inputs<'_>, index: usize) {
        let workspace = inputs.workspaces[index];
        let owner = inputs.owner(index);
        if let Some(owner) = owner {
            self.owner_workspaces
                .entry(owner.into())
                .or_insert_with(|| workspace.id.clone());
            self.owner_project_workspaces
                .entry((owner.into(), workspace.project_id.clone()))
                .or_insert_with(|| workspace.id.clone());
            self.capture_task_workspace(inputs, index, owner);
        }
        self.workspace_owners
            .insert(workspace.id.clone(), owner.map(str::to_owned));
        self.task_paths
            .entry(workspace.root.clone())
            .or_insert_with(|| workspace.id.clone());
        self.task_paths
            .entry(inputs.workspace_roots[index].clone())
            .or_insert_with(|| workspace.id.clone());
    }

    fn capture_task_workspace(&mut self, inputs: &inputs::Inputs<'_>, index: usize, owner: &str) {
        #[cfg(test)]
        crate::workspace::read_work_counts::task_candidate();
        let workspace = inputs.workspaces[index];
        let run = &inputs.runs[inputs.runs_by_owner[owner]];
        let root = &inputs.workspace_roots[index];
        // A direct id may own a different root or project. Task association
        // still requires the old path/project checks and excludes repo roots.
        let matches = run.project == Some(workspace.project_id.as_str())
            && run.root == *root
            && inputs
                .project_roots
                .get(&workspace.project_id)
                .is_some_and(|project| project != root);
        if matches {
            self.task_workspaces
                .entry(owner.into())
                .or_insert_with(|| workspace.id.clone());
        }
    }

    fn capture_run_scopes(&mut self, inputs: &inputs::Inputs<'_>, run: &inputs::RunInput<'_>) {
        #[cfg(test)]
        crate::workspace::read_work_counts::scope_candidate();
        if let Some(index) = inputs.surviving(run) {
            self.surviving_workspaces
                .insert(run.owner.into(), inputs.workspaces[index].id.clone());
        }
        if let Some(workspace) = self.task_paths.get(&run.root).cloned() {
            self.task_paths
                .entry(run.raw_root.to_owned())
                .or_insert(workspace);
        }
        if run.project_owner {
            return;
        }
        let workspace = self
            .owner_workspaces
            .get(run.owner)
            .map(String::as_str)
            .or_else(|| inputs.parked_workspace(run));
        if let Some(workspace) = workspace {
            self.compaction_workspaces
                .insert(run.owner.into(), workspace.into());
        }
    }
}

impl AppState {
    pub(in crate::app) fn capture_snapshot_ownership(&self) -> ProjectionOwnership {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_capture");
        let inputs = inputs::Inputs::capture(self);
        let mut context = ProjectionOwnership {
            rendering: super::projection_rendering::ProjectionRendering::capture(self),
            project_owners: inputs
                .runs
                .iter()
                .filter(|run| run.project_owner)
                .map(|run| run.owner.to_owned())
                .collect(),
            ..ProjectionOwnership::default()
        };
        {
            #[cfg(test)]
            let _phase =
                crate::timing::phase_samples::phase("publication.ownership_workspace_owners");
            for index in 0..inputs.workspaces.len() {
                context.capture_workspace(&inputs, index);
            }
        }
        {
            #[cfg(test)]
            let _phase = crate::timing::phase_samples::phase("publication.ownership_run_scopes");
            for run in &inputs.runs {
                context.capture_run_scopes(&inputs, run);
            }
        }
        context
    }
}
