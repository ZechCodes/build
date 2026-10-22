//! Resolve terminal scopes and enforce immutable source ownership under the
//! app lock. A captured source path asserts location; it never supplies a cwd.

use crate::app::git::optional_scope_id;
use crate::app::{AppState, TabKey};
use crate::changes::ProjectSourceScope;
use crate::lifecycle::PendingState;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) const MOVED_TERMINAL_SOURCE: &str =
    "Build cannot use this project folder: it moved. Reopen Terminal before using it.";
pub(crate) const SOURCE_SHELLS_LIVE: &str = "Close source terminals before moving or removing it";
pub(crate) const SOURCE_TERMINAL_BUSY: &str =
    "The source location is being changed; wait before opening a terminal.";

/// Workspace consoles historically send their selected directory on a
/// workspace-owned shell. That selection never turns it into a source shell.
pub(in crate::app) fn source_scope_requested(params: &Value) -> bool {
    let workspace_only = params
        .get("workspace_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        && params.get("project_id").is_none()
        && params.get("source_path").is_none();
    !workspace_only && (params.get("source_id").is_some() || params.get("source_path").is_some())
}

#[derive(Debug, Clone)]
pub(in crate::app) enum TermScope {
    Run {
        run_id: String,
    },
    ExternalWorktree {
        project_id: String,
        worktree_id: String,
    },
    Primary {
        project_id: String,
    },
    ProjectSource(ProjectSourceScope),
}

impl TermScope {
    pub(in crate::app) fn parse(params: &Value) -> Result<Self, String> {
        if source_scope_requested(params) {
            return Self::project_source(params);
        }
        Self::parse_legacy(params)
    }

    /// Files and old terminal clients retain the existing optional legacy fields.
    pub(in crate::app) fn parse_legacy(params: &Value) -> Result<Self, String> {
        let field = |key| {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        if let Some(run_id) = field("run_id") {
            return Ok(Self::Run { run_id });
        }
        let project_id =
            field("project_id").ok_or("missing scope: run_id or project_id required")?;
        match field("worktree_id") {
            Some(worktree_id) => Ok(Self::ExternalWorktree {
                project_id,
                worktree_id,
            }),
            None => Ok(Self::Primary { project_id }),
        }
    }

    fn project_source(params: &Value) -> Result<Self, String> {
        if [
            "workspace_id",
            "run_id",
            "worktree_id",
            "task_id",
            "root",
            "cwd",
            "path",
            "repo_path",
        ]
        .iter()
        .any(|field| params.get(field).is_some())
        {
            return Err("project source terminal scope cannot be combined with other scope ids or caller roots".into());
        }
        let required = |field| {
            optional_scope_id(params, field)?
                .ok_or_else(|| format!("missing required param: {field}"))
        };
        Ok(Self::ProjectSource(ProjectSourceScope::new(
            required("project_id")?,
            required("source_id")?,
            required("source_path")?,
        )))
    }

    /// Retain checkout roots for legacy file-scope users of this algebra.
    pub(in crate::app) fn resolve_root(&self, state: &mut AppState) -> Result<PathBuf, String> {
        let root = match self {
            Self::ProjectSource(source) => {
                state
                    .resolve_project_source(
                        &source.project_id,
                        &source.source_id,
                        Some(&source.source_path),
                        MOVED_TERMINAL_SOURCE,
                    )?
                    .root
            }
            Self::Run { run_id } => {
                let active = state.runs.get(run_id).ok_or("unknown run_id")?;
                let root = state.run_git_root(run_id, &active.worktree.path);
                if !root.exists() {
                    return Err("worktree no longer exists".into());
                }
                root
            }
            Self::ExternalWorktree {
                project_id,
                worktree_id,
            } => {
                state
                    .resolve_external_worktree(project_id, worktree_id)?
                    .path
            }
            Self::Primary { project_id } => state
                .projects
                .get(project_id)
                .map(|p| p.repo_path.clone())
                .ok_or("unknown project_id")?,
        };
        Ok(AppState::canonical_root(&root))
    }
}

pub(in crate::app) struct TerminalScope {
    pub root: PathBuf,
    pub source: Option<ProjectSourceScope>,
}

impl TerminalScope {
    /// Legacy shells are canonical-root aliases. Explicit source shells also
    /// prove project, source id and the location at which they were opened.
    pub fn accepts(&self, owner: Option<&ProjectSourceScope>) -> bool {
        match (&self.source, owner) {
            (Some(requested), Some(owner)) => requested == owner,
            _ => true,
        }
    }
}

pub(super) fn resolve_terminal_scope(
    state: &mut AppState,
    params: &Value,
) -> Result<TerminalScope, String> {
    // Source parsing runs before the legacy workspace shortcut, including
    // malformed and mixed fields which older parsers would silently ignore.
    let source_requested = source_scope_requested(params);
    if !source_requested {
        if let Some(workspace_id) = params
            .get("workspace_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            return Ok(TerminalScope {
                root: state.workspace_root(workspace_id)?,
                source: None,
            });
        }
    }
    let scope = TermScope::parse(params)?;
    let root = match &scope {
        TermScope::Run { run_id } => {
            let root = &state
                .runs
                .get(run_id)
                .ok_or("unknown run_id")?
                .worktree
                .path;
            if !root.exists() {
                return Err("worktree no longer exists".into());
            }
            AppState::canonical_root(root)
        }
        _ => scope.resolve_root(state)?,
    };
    let source = match scope {
        TermScope::ProjectSource(source) => Some(source),
        _ => None,
    };
    Ok(TerminalScope { root, source })
}

impl AppState {
    pub(in crate::app) fn check_terminal_scope(
        &mut self,
        params: &Value,
        key: &TabKey,
    ) -> Result<(), String> {
        if !["workspace_id", "source_id", "source_path"]
            .iter()
            .any(|field| params.get(field).is_some())
        {
            return Ok(());
        }
        let scope = resolve_terminal_scope(self, params)?;
        if scope.root != key.root
            || (scope.source.is_some() && key.is_agent())
            || !scope.accepts(self.session_registry.shell_source(key))
        {
            // Unknown in this exact scope, even when its wire id exists elsewhere.
            return Err("unknown term_id in requested scope".into());
        }
        Ok(())
    }

    /// Shell reservation and move admission share the app lock. The existing
    /// Updating reservation lasts through off-lock work and its settlement.
    pub(in crate::app) fn refuse_source_shell_start_at(&self, root: &Path) -> Result<(), String> {
        let checkout_id = crate::worktree::external_worktree_id(root);
        if self
            .pending_rows
            .iter()
            .any(|row| row.entity_id == checkout_id && row.state == PendingState::Updating)
        {
            return Err(SOURCE_TERMINAL_BUSY.into());
        }
        Ok(())
    }

    pub(in crate::app) fn refuse_source_shells_at(&self, path: &Path) -> Result<(), String> {
        let root = AppState::canonical_root(path);
        self.refuse_source_shell_start_at(&root)?;
        let shells = self.session_registry.shell_holds_at(&root);
        if !shells.is_empty() {
            return Err(format!("{SOURCE_SHELLS_LIVE}: {}", shells.join(", ")));
        }
        Ok(())
    }
}
