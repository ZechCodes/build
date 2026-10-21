//! Typed native inputs. Hits borrow live rows; only cold capture owns copies.
use super::{inputs, AppState, Workspace};
use crate::agent::AgentRoster;
use crate::run::RunState;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub(super) struct Witness {
    state_root: PathBuf,
    paths: BTreeMap<PathBuf, PathBuf>,
    projects: Vec<(String, PathBuf)>,
    parked: Vec<(Option<String>, Option<String>)>,
    plans: Vec<Roster>,
    runs: Vec<Run>,
    workspaces: Workspaces,
}

struct Roster {
    owner: String,
    agents: Vec<String>,
}

struct Run {
    roster: Roster,
    root: PathBuf,
    project: Option<String>,
    state: RunState,
    recovery_path: Option<String>,
}

struct Workspaces {
    // This is the original name/id-sorted registry order. Validating every
    // member's exact name and id proves this order still holds without sorting.
    ordered: Vec<WorkspaceInput>,
    positions: HashMap<String, usize>,
}

struct WorkspaceInput {
    id: String,
    name: String,
    project: String,
    root: PathBuf,
}

impl Witness {
    pub(super) fn capture(app: &AppState, inputs: &inputs::Inputs<'_>) -> Option<Self> {
        if !app.snapshot_projection_initialized() {
            return None;
        }
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_witness_capture");
        Some(Self {
            state_root: app.state_root.clone(),
            paths: app
                .snapshot_ownership_paths()
                .iter()
                .map(|(raw, observed)| {
                    source_row();
                    (raw.clone(), observed.clone())
                })
                .collect(),
            projects: app
                .projects
                .iter()
                .map(|project| {
                    source_row();
                    (project.id.clone(), project.repo_path.clone())
                })
                .collect(),
            parked: app
                .projects
                .parked()
                .iter()
                .map(|entry| {
                    source_row();
                    (
                        crate::app::projects::configured_primary_path(entry).map(str::to_owned),
                        entry["id"].as_str().map(str::to_owned),
                    )
                })
                .collect(),
            plans: app
                .plans
                .iter()
                .map(|(owner, plan)| Roster::capture(owner, &plan.agents))
                .collect(),
            runs: app
                .runs
                .iter()
                .map(|(owner, active)| Run::capture(app, owner, active))
                .collect(),
            workspaces: Workspaces::capture(&inputs.workspaces),
        })
    }

    pub(super) fn matches(&self, app: &AppState) -> bool {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_validate");
        app.snapshot_projection_initialized()
            && self.state_root == app.state_root
            && self.paths.iter().eq(app
                .snapshot_ownership_paths()
                .iter()
                .inspect(|_| source_row()))
            && self
                .projects
                .iter()
                .map(|(id, path)| (id.as_str(), path.as_path()))
                .eq(app.projects.iter().map(|project| {
                    source_row();
                    (project.id.as_str(), project.repo_path.as_path())
                }))
            && self
                .parked
                .iter()
                .map(|(path, id)| (path.as_deref(), id.as_deref()))
                .eq(app.projects.parked().iter().map(|entry| {
                    source_row();
                    (
                        crate::app::projects::configured_primary_path(entry),
                        entry["id"].as_str(),
                    )
                }))
            && self.plans.len() == app.plans.len()
            && self
                .plans
                .iter()
                .zip(app.plans.iter())
                .all(|(before, (owner, plan))| before.matches(owner, &plan.agents))
            && self.runs.len() == app.runs.len()
            && self
                .runs
                .iter()
                .zip(app.runs.iter())
                .all(|(before, (owner, active))| before.matches(app, owner, active))
            && self.workspaces.matches(app)
    }
}

impl Roster {
    fn capture(owner: &str, agents: &AgentRoster) -> Self {
        source_row();
        Self {
            owner: owner.into(),
            agents: agents
                .iter()
                .map(|agent| {
                    source_row();
                    agent.id.clone()
                })
                .collect(),
        }
    }

    fn matches(&self, owner: &str, agents: &AgentRoster) -> bool {
        source_row();
        self.owner == owner
            && self
                .agents
                .iter()
                .map(String::as_str)
                .eq(agents.iter().map(|agent| {
                    source_row();
                    agent.id.as_str()
                }))
    }
}

impl Run {
    fn capture(app: &AppState, owner: &str, active: &crate::orchestrator::ActiveRun) -> Self {
        let project = app.projects.project_id_of(owner);
        // For registered projects, native repo paths above already cover the
        // scratch input. Only an absent project needs retained recovery text.
        let recovery_path = project
            .and_then(|id| app.projects.get(id))
            .is_none()
            .then(|| app.projects.project_path_for(owner));
        Self {
            roster: Roster::capture(owner, &active.agents),
            root: active.worktree.path.clone(),
            project: project.map(str::to_owned),
            state: active.run.state,
            recovery_path,
        }
    }

    fn matches(
        &self,
        app: &AppState,
        owner: &str,
        active: &crate::orchestrator::ActiveRun,
    ) -> bool {
        self.roster.matches(owner, &active.agents)
            && self.root == active.worktree.path
            && self.project.as_deref() == app.projects.project_id_of(owner)
            && self.state == active.run.state
            && self
                .recovery_path
                .as_ref()
                .is_none_or(|path| *path == app.projects.project_path_for(owner))
    }
}

impl Workspaces {
    fn capture(workspaces: &[&Workspace]) -> Self {
        let ordered = workspaces
            .iter()
            .map(|workspace| {
                source_row();
                WorkspaceInput {
                    id: workspace.id.clone(),
                    name: workspace.name.clone(),
                    project: workspace.project_id.clone(),
                    root: workspace.root.clone(),
                }
            })
            .collect::<Vec<_>>();
        let positions = ordered
            .iter()
            .enumerate()
            .map(|(index, workspace)| (workspace.id.clone(), index))
            .collect();
        Self { ordered, positions }
    }

    fn matches(&self, app: &AppState) -> bool {
        let mut current = app.workspaces.iter();
        self.ordered.len() == current.len()
            && current.all(|workspace| {
                source_row();
                self.positions.get(&workspace.id).is_some_and(|index| {
                    let before = &self.ordered[*index];
                    before.id == workspace.id
                        && before.name == workspace.name
                        && before.project == workspace.project_id
                        && before.root == workspace.root
                })
            })
    }
}

fn source_row() {
    #[cfg(test)]
    super::super::work_counts::source_row();
}
