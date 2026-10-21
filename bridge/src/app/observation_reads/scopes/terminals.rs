//! Terminal scope resolution observes native roots on the existing I/O lane.
use super::*;
use crate::app::runtime::terminals::{
    shell_tab_rows, source_scope_requested, TermScope, TerminalScope, MOVED_TERMINAL_SOURCE,
};

/// Terminal runs use the checkout container even when Git uses a child source.
/// Workspace and explicit source roots retain their strict canonicalization.
enum TerminalRoot {
    Canonical(PathBuf),
    Checkout(PathBuf),
    Strict(PathBuf, String),
}

impl TerminalRoot {
    fn resolve(self) -> Result<PathBuf, String> {
        match self {
            Self::Canonical(path) => Ok(AppState::canonical_root(&path)),
            Self::Checkout(path) => {
                if !path.exists() {
                    return Err("worktree no longer exists".into());
                }
                Ok(AppState::canonical_root(&path))
            }
            Self::Strict(path, prefix) => {
                std::fs::canonicalize(path).map_err(|error| format!("{prefix}: {error}"))
            }
        }
    }
}

impl ScopeReadContext {
    pub(super) fn prepare_terminal_list(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let mut fence = ScopeReadFence::new(MOVED_TERMINAL_SOURCE);
        let (root, source) = self.terminal_root(params, &mut fence)?;
        let tabs = self.terminals.clone();
        Ok(ScopeReadJob::terminal(fence, params.clone(), move || {
            let scope = TerminalScope {
                root: root.resolve()?,
                source,
            };
            Ok(serde_json::json!({"terminals": shell_tab_rows(&tabs, &scope)}))
        }))
    }

    fn terminal_root(
        &self,
        params: &Value,
        fence: &mut ScopeReadFence,
    ) -> Result<(TerminalRoot, Option<crate::changes::ProjectSourceScope>), String> {
        // Preserve source parsing before the legacy workspace shortcut. A
        // workspace's selected source_id alone never narrows its shell root.
        if !source_scope_requested(params) {
            if let Some(id) = params
                .get("workspace_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                let workspace = self
                    .workspaces
                    .get(id)
                    .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
                fence.bindings.push(Binding::WorkspaceRoot {
                    id: id.into(),
                    root: workspace.root.clone(),
                });
                return Ok((
                    TerminalRoot::Strict(
                        workspace.root.clone(),
                        format!("resolve workspace root {}", workspace.root.display()),
                    ),
                    None,
                ));
            }
        }
        match TermScope::parse(params)? {
            TermScope::ProjectSource(scope) => {
                let source = self.source(&scope.project_id, &scope.source_id)?;
                if scope.source_path != source.path.display().to_string() {
                    return Err(MOVED_TERMINAL_SOURCE.into());
                }
                fence
                    .bindings
                    .push(self.source_binding(&scope.project_id, &scope.source_id)?);
                Ok((
                    TerminalRoot::Strict(source.path.clone(), "cannot resolve scope root".into()),
                    Some(scope),
                ))
            }
            TermScope::Run { run_id } => {
                let checkout = self
                    .runs
                    .get(&run_id)
                    .ok_or("unknown run_id")?
                    .checkout
                    .clone();
                fence.bindings.push(Binding::TerminalRun {
                    id: run_id,
                    checkout: checkout.clone(),
                });
                Ok((TerminalRoot::Checkout(checkout), None))
            }
            TermScope::Primary { project_id } => {
                let project = self.projects.get(&project_id).ok_or("unknown project_id")?;
                fence.bindings.push(self.project_binding(&project_id)?);
                Ok((TerminalRoot::Canonical(project.path.clone()), None))
            }
            TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            } => {
                let worktree = self.terminal_external(&project_id, &worktree_id)?;
                fence.bindings.push(self.project_binding(&project_id)?);
                fence
                    .bindings
                    .push(self.external_binding(&project_id, &worktree));
                Ok((TerminalRoot::Canonical(worktree.path), None))
            }
        }
    }

    fn terminal_external(&self, project: &str, id: &str) -> Result<ExternalWorktree, String> {
        if !self.projects.contains_key(project) {
            return Err(format!(
                "unknown worktree_id: {id} ({})",
                crate::app::board::cache::scan_may_yet_show_it(false)
            ));
        }
        self.external(project, id)
    }
}
