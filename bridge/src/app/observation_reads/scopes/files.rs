use super::*;
use crate::app::fs::{
    directory_listing, host_directory_list, read_scoped_file, FileScope, MOVED_SOURCE,
};
use crate::app::TermScope;

impl ScopeReadContext {
    pub(super) fn prepare_file(
        &self,
        method: &str,
        params: &Value,
    ) -> Result<ScopeReadJob, String> {
        let mut fence = ScopeReadFence::new(MOVED_SOURCE);
        let params = params.clone();
        if method == "fs.list" {
            return Ok(ScopeReadJob::new(fence, move || {
                host_directory_list(&params)
            }));
        }
        let scope = FileScope::parse(&params)?;
        let (root, strict, needs_existing) = self.file_root(&scope, &mut fence)?;
        let tree = method == "fs.tree";
        Ok(ScopeReadJob::new(fence, move || {
            if needs_existing && !root.exists() {
                return Err("worktree no longer exists".into());
            }
            let root = match strict {
                Some(prefix) => {
                    std::fs::canonicalize(&root).map_err(|error| format!("{prefix}: {error}"))?
                }
                None => AppState::canonical_root(&root),
            };
            if tree {
                directory_listing(
                    &root,
                    params.get("path").and_then(Value::as_str).unwrap_or(""),
                )
            } else {
                read_scoped_file(&root, &params)
            }
        }))
    }

    fn file_root(
        &self,
        scope: &FileScope,
        fence: &mut ScopeReadFence,
    ) -> Result<(PathBuf, Option<String>, bool), String> {
        match scope {
            FileScope::WorkspaceSource {
                workspace_id,
                source_id,
            } => {
                let (workspace, directory) = self.directory(workspace_id, source_id)?;
                fence
                    .bindings
                    .push(self.directory_binding(workspace, directory));
                Ok((
                    directory.path.clone(),
                    Some(format!(
                        "resolve workspace root {}",
                        directory.path.display()
                    )),
                    false,
                ))
            }
            FileScope::ProjectSource {
                project_id,
                source_id,
                source_path,
            } => {
                let source = self.source(project_id, source_id)?;
                if source_path
                    .as_ref()
                    .is_some_and(|opened| *opened != source.path.display().to_string())
                {
                    return Err(MOVED_SOURCE.into());
                }
                fence
                    .bindings
                    .push(self.source_binding(project_id, source_id)?);
                Ok((
                    source.path.clone(),
                    Some("cannot resolve scope root".into()),
                    false,
                ))
            }
            FileScope::Legacy(TermScope::Run { run_id }) => {
                let run = self.run(run_id)?;
                fence.bindings.push(self.run_binding(run_id)?);
                Ok((run.git_root.clone(), None, true))
            }
            FileScope::Legacy(TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            }) => {
                let worktree = self.external(project_id, worktree_id)?;
                fence.bindings.push(self.project_binding(project_id)?);
                fence
                    .bindings
                    .push(self.external_binding(project_id, &worktree));
                Ok((worktree.path, None, false))
            }
            FileScope::Legacy(TermScope::Primary { project_id }) => {
                fence.bindings.push(self.project_binding(project_id)?);
                Ok((self.project(project_id)?.path.clone(), None, false))
            }
            FileScope::Legacy(TermScope::ProjectSource(_)) => {
                unreachable!("file parser handles explicit project sources")
            }
        }
    }

    pub(super) fn prepare_git_init(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let id = crate::app::require_str(params, "workspace_id")?;
        let source = crate::app::require_str(params, "source_id")?;
        let (workspace, directory) = self.directory(&id, &source)?;
        let mut fence = ScopeReadFence::new(
            "workspace directory changed while reading Git initialization options",
        );
        fence
            .bindings
            .push(self.directory_binding(workspace, directory));
        let source = self
            .source(&workspace.project_id, &directory.source_id)
            .cloned();
        if let Ok(source) = &source {
            fence
                .bindings
                .push(self.source_binding(&workspace.project_id, &source.id)?);
        }
        let workspace = workspace.clone();
        let directory = directory.clone();
        Ok(ScopeReadJob::new(fence, move || {
            let source = source.as_ref().map_err(Clone::clone);
            Ok(
                crate::app::workspaces::git_initialization::observe_git_init_options(
                    &workspace, &directory, source,
                ),
            )
        }))
    }
}
