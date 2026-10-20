//! Owned checkout identities for filesystem and Git observations. Capture only
//! copies committed rows and already-warmed paths; jobs never retain AppState.

use crate::api::ApiError;
use crate::app::projects::ProjectSource;
use crate::app::{AppState, ReadSubject};
use crate::run::StageProgress;
use crate::workspace::Workspace;
use crate::worktree::ExternalWorktree;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

mod files;
mod git;
mod resolution;
mod stages;

pub(in crate::app) const METHODS: &[&str] = &[
    "git.log",
    "git.show",
    "git.status",
    "git.diff",
    "git.refs",
    "git.unpushed",
    "git.changeset_diff",
    "worktree.diff",
    "run.diff",
    "task.stage_diff",
    "fs.list",
    "fs.tree",
    "fs.read",
    "workspace.git_init_options",
];

#[derive(Default)]
pub(in crate::app) struct ScopeReadContext {
    projects: BTreeMap<String, ProjectInput>,
    workspaces: BTreeMap<String, Workspace>,
    runs: BTreeMap<String, RunInput>,
    lineages: BTreeMap<String, Vec<String>>,
}

struct ProjectInput {
    path: PathBuf,
    is_git: bool,
    base_branch: String,
    sources: Vec<ProjectSource>,
    external: Vec<ExternalWorktree>,
    scan_settled: bool,
}

struct RunInput {
    project_id: Option<String>,
    project_path: PathBuf,
    checkout: PathBuf,
    git_root: PathBuf,
    base_sha: Option<String>,
    base_branch: String,
    stages: Vec<StageProgress>,
    terminal: bool,
}

#[derive(Clone)]
pub(in crate::app) struct ScopeReadFence {
    bindings: Vec<Binding>,
    moved: &'static str,
}

#[derive(Clone)]
enum Binding {
    Project {
        id: String,
        path: PathBuf,
        is_git: bool,
    },
    Source {
        project: String,
        id: String,
        path: PathBuf,
        is_git: bool,
    },
    Directory {
        workspace: String,
        root: PathBuf,
        id: String,
        source: String,
        path: PathBuf,
        is_git: bool,
    },
    External {
        project: String,
        id: String,
        path: PathBuf,
    },
    Run {
        id: String,
        checkout: PathBuf,
        git_root: PathBuf,
    },
    Stage {
        run: String,
        stage: StageProgress,
    },
}

pub(in crate::app) struct ScopeReadJob {
    work: Box<dyn FnOnce() -> Result<Value, String> + Send>,
    fence: ScopeReadFence,
}

impl ScopeReadJob {
    fn new(
        fence: ScopeReadFence,
        work: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) -> Self {
        Self {
            work: Box::new(work),
            fence,
        }
    }

    pub fn fence(&self) -> ScopeReadFence {
        self.fence.clone()
    }

    pub fn run(self) -> Result<Value, ApiError> {
        (self.work)().map_err(ApiError::classify)
    }
}

impl ScopeReadFence {
    fn new(moved: &'static str) -> Self {
        Self {
            bindings: Vec::new(),
            moved,
        }
    }
}

impl ScopeReadContext {
    pub fn capture(app: &AppState) -> Self {
        let projects = app
            .projects
            .iter()
            .map(|project| {
                let scan = app.board.diff().external_scan(&project.id);
                (
                    project.id.clone(),
                    ProjectInput {
                        path: project.repo_path.clone(),
                        is_git: project.is_git,
                        base_branch: project.base_branch.clone(),
                        sources: project.sources.clone(),
                        external: scan.worktrees.to_vec(),
                        scan_settled: scan.settled_at.is_some(),
                    },
                )
            })
            .collect();
        let workspaces = app
            .workspaces
            .list(None)
            .into_iter()
            .map(|workspace| (workspace.id.clone(), workspace.clone()))
            .collect();
        let runs = app
            .runs
            .iter()
            .map(|(id, run)| {
                let git_root = if !app.snapshot_projection_initialized() {
                    // No observation is admitted before warming. Cold test roots
                    // can still carry empty, entirely in-memory contexts.
                    run.worktree.path.clone()
                } else if app.snapshot_path_exists(&run.worktree.path.join(".git")) {
                    run.worktree.path.clone()
                } else {
                    app.workspace_snapshot_for_conversation(id, None)
                        .and_then(|workspace| {
                            workspace
                                .directories
                                .iter()
                                .find(|directory| directory.is_git)
                        })
                        .map(|directory| directory.path.clone())
                        .unwrap_or_else(|| run.worktree.path.clone())
                };
                (
                    id.clone(),
                    RunInput {
                        project_id: app.projects.project_id_of(id).map(str::to_owned),
                        project_path: PathBuf::from(app.project_path_for(id)),
                        checkout: run.worktree.path.clone(),
                        git_root,
                        base_sha: run.base_sha.clone(),
                        base_branch: app.run_base_branch(id, &run.worktree.base_branch),
                        stages: run.stages.clone(),
                        terminal: run.run.state.is_terminal(),
                    },
                )
            })
            .collect();
        let mut ordered = app.runs.values().collect::<Vec<_>>();
        ordered.sort_by_key(|run| {
            app.board
                .attention()
                .clock(&run.run.id.0)
                .created_at
                .unwrap_or_default()
        });
        let mut lineages = BTreeMap::<String, Vec<String>>::new();
        for run in ordered {
            if let Some(task) = &run.run.plan_id {
                lineages
                    .entry(task.0.clone())
                    .or_default()
                    .push(run.run.id.0.clone());
            }
        }
        Self {
            projects,
            workspaces,
            runs,
            lineages,
        }
    }

    pub fn prepare(&self, method: &str, params: &Value) -> Result<ScopeReadJob, ApiError> {
        self.prepare_scoped(method, params)
            .map_err(ApiError::classify)
    }

    fn prepare_scoped(&self, method: &str, params: &Value) -> Result<ScopeReadJob, String> {
        match method {
            "fs.list" | "fs.tree" | "fs.read" => self.prepare_file(method, params),
            "workspace.git_init_options" => self.prepare_git_init(params),
            "worktree.diff" => self.prepare_worktree_diff(params),
            "run.diff" => self.prepare_run_diff(params),
            "task.stage_diff" => self.prepare_stage_diff(params),
            "git.changeset_diff" => self.prepare_changeset(params),
            _ => self.prepare_git(method, params),
        }
    }

    fn diff_job(
        subject: ReadSubject,
        task_id: Option<String>,
        params: &Value,
        fence: ScopeReadFence,
    ) -> ScopeReadJob {
        let read = crate::app::runtime::deferred::DeferredRead {
            subject,
            task_id,
            if_diff_key: params
                .get("if_diff_key")
                .and_then(Value::as_str)
                .map(str::to_owned),
            with_patch: crate::app::wants_patch(params),
            paths: None,
            range: None,
            #[cfg(test)]
            gate: None,
        };
        ScopeReadJob::new(fence, move || read.run())
    }
}
