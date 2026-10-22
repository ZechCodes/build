//! Owned checkout identities for filesystem and Git observations. Capture only
//! copies committed rows and already-warmed paths; jobs never retain AppState.

use crate::api::ApiError;
use crate::app::projects::ProjectSource;
use crate::app::{AppState, ReadSubject};
use crate::run::StageProgress;
use crate::workspace::Workspace;
use crate::worktree::ExternalWorktree;
use imbl::OrdMap;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

mod capture;
mod files;
mod git;
mod resolution;
mod stages;
mod terminals;

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
    "term.list",
];

#[derive(Clone, Default)]
pub(in crate::app) struct ScopeReadContext {
    terminal_generation: u64,
    terminals: Arc<Vec<crate::app::runtime::sessions::ShellTabFact>>,
    projects: OrdMap<String, Arc<ProjectInput>>,
    plan_projects: OrdMap<String, String>,
    workspaces: OrdMap<String, Arc<Workspace>>,
    runs: OrdMap<String, Arc<RunInput>>,
    lineages: OrdMap<String, Arc<Vec<String>>>,
    order: Arc<BTreeMap<String, usize>>,
    workspace_dependents: OrdMap<String, imbl::OrdSet<String>>,
    workspace_roots: OrdMap<PathBuf, imbl::OrdSet<String>>,
    workspace_canonical: OrdMap<String, PathBuf>,
}

pub(super) use capture::ScopePatch;

struct ProjectInput {
    path: PathBuf,
    is_git: bool,
    base_branch: String,
    sources: Vec<ProjectSource>,
    external: Vec<ExternalWorktree>,
    scan_settled: bool,
}

#[derive(Clone)]
struct RunInput {
    plan_id: Option<String>,
    created_at: String,
    workspace_id: Option<String>,
    checkout_git: bool,
    canonical_checkout: PathBuf,
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
    TerminalRun {
        id: String,
        checkout: PathBuf,
    },
    WorkspaceRoot {
        id: String,
        root: PathBuf,
    },
    Stage {
        run: String,
        stage: StageProgress,
    },
}

pub(in crate::app) struct ScopeReadJob {
    work: Box<dyn FnOnce() -> Result<Value, ApiError> + Send>,
    fence: ScopeReadFence,
}

impl ScopeReadJob {
    fn new(
        fence: ScopeReadFence,
        work: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) -> Self {
        Self {
            work: Box::new(move || work().map_err(ApiError::classify)),
            fence,
        }
    }

    fn terminal(
        fence: ScopeReadFence,
        params: Value,
        work: impl FnOnce() -> Result<Value, String> + Send + 'static,
    ) -> Self {
        Self {
            work: Box::new(move || {
                work().map_err(|error| {
                    crate::app::rpc::terminal_session_refusal("term.list", &params, error)
                })
            }),
            fence,
        }
    }

    pub fn fence(&self) -> ScopeReadFence {
        self.fence.clone()
    }

    pub fn run(self) -> Result<Value, ApiError> {
        (self.work)()
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
    pub fn prepare(&self, method: &str, params: &Value) -> Result<ScopeReadJob, ApiError> {
        self.prepare_scoped(method, params).map_err(|error| {
            if method == "term.list" {
                crate::app::rpc::terminal_session_refusal(method, params, error)
            } else {
                ApiError::classify(error)
            }
        })
    }

    fn prepare_scoped(&self, method: &str, params: &Value) -> Result<ScopeReadJob, String> {
        match method {
            "fs.list" | "fs.tree" | "fs.read" => self.prepare_file(method, params),
            "workspace.git_init_options" => self.prepare_git_init(params),
            "term.list" => self.prepare_terminal_list(params),
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

impl ScopeReadContext {
    pub(in crate::app) fn mcp_project_binding(&self, owner: &str) -> Option<&str> {
        self.runs
            .get(owner)
            .and_then(|run| run.project_id.as_deref())
            .or_else(|| self.plan_projects.get(owner).map(String::as_str))
    }
    pub(in crate::app) fn mcp_project_path(&self, project: &str) -> Option<&std::path::Path> {
        self.projects.get(project).map(|p| p.path.as_path())
    }
    pub(in crate::app) fn mcp_workspace_project(&self, id: &str) -> Option<&str> {
        self.workspaces.get(id).map(|w| w.project_id.as_str())
    }
}
