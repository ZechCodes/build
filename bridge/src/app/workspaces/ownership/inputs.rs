//! One ordered registry observation plus indexes preserving the old scans' winners.
use super::{AppState, Workspace};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) struct RunInput<'a> {
    pub owner: &'a str,
    pub root: PathBuf,
    pub raw_root: &'a Path,
    pub project: Option<&'a str>,
    pub terminal: bool,
    pub project_owner: bool,
    pub parked_project: Option<String>,
}

pub(super) struct Inputs<'a> {
    pub runs: Vec<RunInput<'a>>,
    pub workspaces: Vec<&'a Workspace>,
    pub workspace_roots: Vec<PathBuf>,
    pub project_roots: HashMap<String, PathBuf>,
    pub runs_by_owner: HashMap<&'a str, usize>,
    pub first_live: HashMap<(String, PathBuf), &'a str>,
    pub first_workspace: HashMap<(String, PathBuf), usize>,
    pub workspace_ids: HashMap<&'a str, usize>,
}

#[derive(Default)]
struct Roots(HashMap<PathBuf, PathBuf>);

impl Roots {
    fn get(&mut self, app: &AppState, path: &Path) -> PathBuf {
        self.0
            .entry(path.to_owned())
            .or_insert_with(|| app.snapshot_root(path))
            .clone()
    }
}

impl<'a> Inputs<'a> {
    pub(super) fn capture(app: &'a AppState) -> Self {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_inputs");
        let mut roots = Roots::default();
        let runs = capture_runs(app, &mut roots);
        let mut captured = Self {
            runs,
            workspaces: app.workspaces.list(None),
            workspace_roots: Vec::new(),
            project_roots: app
                .projects
                .iter()
                .map(|project| (project.id.clone(), roots.get(app, &project.repo_path)))
                .collect(),
            runs_by_owner: HashMap::new(),
            first_live: HashMap::new(),
            first_workspace: HashMap::new(),
            workspace_ids: HashMap::new(),
        };
        for (index, run) in captured.runs.iter().enumerate() {
            captured.runs_by_owner.insert(run.owner, index);
            if !run.terminal && !run.project_owner {
                if let Some(project) = run.project {
                    captured
                        .first_live
                        .entry((project.into(), run.root.clone()))
                        .or_insert(run.owner);
                }
            }
        }
        for (index, workspace) in captured.workspaces.iter().enumerate() {
            #[cfg(test)]
            crate::workspace::read_work_counts::scope_candidate();
            let root = roots.get(app, &workspace.root);
            captured
                .first_workspace
                .entry((workspace.project_id.clone(), root.clone()))
                .or_insert(index);
            captured.workspace_ids.insert(&workspace.id, index);
            captured.workspace_roots.push(root);
        }
        captured
    }

    pub(super) fn owner(&self, index: usize) -> Option<&str> {
        let workspace = self.workspaces[index];
        if let Some(run) = self
            .runs_by_owner
            .get(workspace.id.as_str())
            .map(|i| &self.runs[*i])
        {
            if !run.project_owner {
                return Some(run.owner);
            }
        }
        self.first_live
            .get(&(
                workspace.project_id.clone(),
                self.workspace_roots[index].clone(),
            ))
            .copied()
    }

    pub(super) fn surviving(&self, run: &RunInput<'_>) -> Option<usize> {
        let project = run.project?;
        let root_match = self
            .first_workspace
            .get(&(project.into(), run.root.clone()))
            .copied();
        let direct = self
            .workspace_ids
            .get(run.owner)
            .copied()
            .filter(|index| self.workspaces[*index].project_id == project);
        root_match.into_iter().chain(direct).min()
    }

    pub(super) fn parked_workspace(&self, run: &RunInput<'_>) -> Option<&str> {
        if run.project.is_some() || run.terminal {
            return None;
        }
        let project = run.parked_project.as_ref()?;
        self.first_workspace
            .get(&(project.clone(), run.root.clone()))
            .map(|index| self.workspaces[*index].id.as_str())
    }
}

fn capture_runs<'a>(app: &'a AppState, roots: &mut Roots) -> Vec<RunInput<'a>> {
    let mut parked = HashMap::new();
    for entry in app.projects.parked() {
        if let Some(path) = crate::app::projects::configured_primary_path(entry) {
            parked
                .entry(path)
                .or_insert_with(|| entry["id"].as_str().map(str::to_owned));
        }
    }
    let projects: HashMap<_, _> = app
        .projects
        .iter()
        .map(|project| (project.id.as_str(), project.repo_path.as_path()))
        .collect();
    // HashMap iteration is intentional: the first live fallback owner in
    // that exact traversal has always won shared-root workspaces.
    app.runs
        .iter()
        .map(|(owner, active)| {
            #[cfg(test)]
            crate::workspace::read_work_counts::owner_candidate();
            let root = roots.get(app, &active.worktree.path);
            let project = app.projects.project_id_of(owner);
            let retained = if project.is_none() {
                app.projects.project_path_for(owner)
            } else {
                String::new()
            };
            let project_path = project
                .and_then(|id| projects.get(id))
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(app.projects.project_path_for(owner)));
            let project_owner = !project_path.as_os_str().is_empty()
                && root
                    == roots.get(
                        app,
                        &crate::app::projects::scratch_dir(&app.state_root, &project_path),
                    );
            RunInput {
                owner,
                root,
                raw_root: &active.worktree.path,
                project,
                terminal: active.run.state.is_terminal(),
                project_owner,
                parked_project: parked.get(retained.as_str()).cloned().flatten(),
            }
        })
        .collect()
}
