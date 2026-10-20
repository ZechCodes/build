use super::*;
use crate::app::git::{GitScope, GitScopeRun, GitScopeWorktree};
use crate::app::projects::{ResolvedProjectSource, MOVED_GIT_SOURCE, NOT_GIT_SOURCE};
use crate::workspace::WorkspaceDirectory;

impl ScopeReadContext {
    pub(super) fn project(&self, id: &str) -> Result<&ProjectInput, String> {
        self.projects
            .get(id)
            .ok_or_else(|| format!("unknown project_id: {id}"))
    }

    pub(super) fn source(&self, project: &str, id: &str) -> Result<&ProjectSource, String> {
        self.project(project)?
            .sources
            .iter()
            .find(|source| source.id == id)
            .ok_or_else(|| format!("unknown source_id {id} in project {project}"))
    }

    pub(super) fn directory(
        &self,
        workspace: &str,
        source: &str,
    ) -> Result<(&Workspace, &WorkspaceDirectory), String> {
        let workspace = self
            .workspaces
            .get(workspace)
            .ok_or_else(|| format!("unknown workspace_id: {workspace}"))?;
        let directory = workspace
            .directories
            .iter()
            .find(|directory| directory.source_id == source || directory.id == source)
            .ok_or_else(|| format!("unknown source_id {source} in workspace {}", workspace.id))?;
        Ok((workspace, directory))
    }

    pub(super) fn run(&self, id: &str) -> Result<&RunInput, String> {
        self.runs.get(id).ok_or_else(|| "unknown run_id".into())
    }

    pub(super) fn external(&self, project: &str, id: &str) -> Result<ExternalWorktree, String> {
        let project = self.project(project)?;
        project
            .external
            .iter()
            .find(|worktree| worktree.id == id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "unknown worktree_id: {id} ({})",
                    crate::app::board::cache::scan_may_yet_show_it(project.scan_settled)
                )
            })
    }

    pub(super) fn external_binding(&self, project: &str, worktree: &ExternalWorktree) -> Binding {
        Binding::External {
            project: project.into(),
            id: worktree.id.clone(),
            path: worktree.path.clone(),
        }
    }

    pub(super) fn project_binding(&self, id: &str) -> Result<Binding, String> {
        let project = self.project(id)?;
        Ok(Binding::Project {
            id: id.into(),
            path: project.path.clone(),
            is_git: project.is_git,
        })
    }

    pub(super) fn source_binding(&self, project: &str, id: &str) -> Result<Binding, String> {
        let source = self.source(project, id)?;
        Ok(Binding::Source {
            project: project.into(),
            id: id.into(),
            path: source.path.clone(),
            is_git: source.is_git,
        })
    }

    pub(super) fn directory_binding(
        &self,
        workspace: &Workspace,
        directory: &WorkspaceDirectory,
    ) -> Binding {
        Binding::Directory {
            workspace: workspace.id.clone(),
            root: workspace.root.clone(),
            id: directory.id.clone(),
            source: directory.source_id.clone(),
            path: directory.path.clone(),
            is_git: directory.is_git,
        }
    }

    pub(super) fn run_binding(&self, id: &str) -> Result<Binding, String> {
        let run = self.run(id)?;
        Ok(Binding::Run {
            id: id.into(),
            checkout: run.checkout.clone(),
            git_root: run.git_root.clone(),
        })
    }

    pub(super) fn git_scope(
        &self,
        params: &Value,
        fence: &mut ScopeReadFence,
    ) -> Result<GitScope, String> {
        if let Some(scope) = self.source_git_scope(params, fence)? {
            return Ok(scope);
        }
        let project = params.get("project_id").and_then(Value::as_str);
        let run = params.get("run_id").and_then(Value::as_str);
        let worktree = params.get("worktree_id").and_then(Value::as_str);
        match (project, run, worktree) {
            (Some(project), None, Some(worktree)) => {
                let external = self.external(project, worktree)?;
                fence.bindings.push(self.project_binding(project)?);
                fence
                    .bindings
                    .push(self.external_binding(project, &external));
                Ok(GitScope {
                    repo_path: external.path,
                    cache_namespace: None,
                    marks_unpushed: false,
                    project_id: None,
                    source: None,
                    run: None,
                    worktree: Some(GitScopeWorktree {
                        project_id: project.into(),
                        base_branch: self.project(project)?.base_branch.clone(),
                    }),
                })
            }
            (Some(id), None, None) => {
                let project = self.project(id)?;
                if !project.is_git {
                    return Err("project is not a git repository; initialize Git first".into());
                }
                fence.bindings.push(self.project_binding(id)?);
                Ok(GitScope {
                    repo_path: project.path.clone(),
                    cache_namespace: None,
                    marks_unpushed: false,
                    project_id: Some(id.into()),
                    source: None,
                    run: None,
                    worktree: None,
                })
            }
            (None, Some(id), None) => {
                let run = self.run(id)?;
                fence.bindings.push(self.run_binding(id)?);
                Ok(GitScope {
                    repo_path: run.git_root.clone(),
                    cache_namespace: None,
                    marks_unpushed: false,
                    project_id: None,
                    source: None,
                    worktree: None,
                    run: Some(GitScopeRun {
                        run_id: id.into(),
                        base_branch: run.base_branch.clone(),
                    }),
                })
            }
            _ => {
                Err("provide exactly one of project_id, run_id, or project_id + worktree_id".into())
            }
        }
    }

    pub(super) fn source_git_scope(
        &self,
        params: &Value,
        fence: &mut ScopeReadFence,
    ) -> Result<Option<GitScope>, String> {
        let workspace = crate::app::git::optional_scope_id(params, "workspace_id")?;
        let source = crate::app::git::optional_scope_id(params, "source_id")?;
        let path = crate::app::git::optional_scope_id(params, "source_path")?;
        match (workspace, source) {
            (None, Some(source)) if params.get("project_id").is_some() => self
                .project_source_git(params, &source, path, fence)
                .map(Some),
            _ if path.is_some() => Err("source_path must name a project source scope".into()),
            (None, None) => Ok(None),
            (workspace, source) => {
                let workspace = workspace.ok_or("missing required param: workspace_id")?;
                let source = source.ok_or("missing required param: source_id")?;
                if ["project_id", "run_id", "worktree_id", "task_id"]
                    .iter()
                    .any(|key| params.get(key).is_some())
                {
                    return Err(
                        "workspace source scope cannot be combined with a legacy git scope".into(),
                    );
                }
                let (workspace, directory) = self.directory(&workspace, &source)?;
                if !directory.is_git {
                    return Err("workspace source is not a git repository".into());
                }
                fence
                    .bindings
                    .push(self.directory_binding(workspace, directory));
                Ok(Some(GitScope {
                    repo_path: directory.path.clone(),
                    cache_namespace: Some(format!(
                        "workspace:{}:directory:{}",
                        workspace.id, directory.id
                    )),
                    marks_unpushed: true,
                    project_id: None,
                    run: None,
                    worktree: None,
                    source: None,
                }))
            }
        }
    }

    fn project_source_git(
        &self,
        params: &Value,
        source_id: &str,
        path: Option<String>,
        fence: &mut ScopeReadFence,
    ) -> Result<GitScope, String> {
        if ["run_id", "worktree_id", "task_id"]
            .iter()
            .any(|key| params.get(key).is_some())
        {
            return Err(
                "project source scope cannot be combined with run or worktree scope ids".into(),
            );
        }
        let project = crate::app::git::optional_scope_id(params, "project_id")?
            .ok_or("missing required param: project_id")?;
        let path = path.ok_or("missing required param: source_path")?;
        let source = self.source(&project, source_id)?;
        if path != source.path.display().to_string() {
            return Err(MOVED_GIT_SOURCE.into());
        }
        if !source.is_git {
            return Err(NOT_GIT_SOURCE.into());
        }
        fence
            .bindings
            .push(self.source_binding(&project, source_id)?);
        let source = ResolvedProjectSource {
            project_id: project.clone(),
            source_id: source_id.into(),
            source_path: path,
            root: source.path.clone(),
            is_git: source.is_git,
        };
        Ok(GitScope {
            repo_path: source.root.clone(),
            cache_namespace: Some(crate::app::git::project_source_namespace(&source)),
            marks_unpushed: false,
            project_id: Some(project),
            run: None,
            worktree: None,
            source: Some(source),
        })
    }

    pub fn validate(&self, fence: &ScopeReadFence) -> Result<(), ApiError> {
        for binding in &fence.bindings {
            let current = self
                .binding_is_current(binding)
                .map_err(ApiError::classify)?;
            if !current {
                return Err(ApiError::conflict(fence.moved, None));
            }
        }
        Ok(())
    }

    fn binding_is_current(&self, binding: &Binding) -> Result<bool, String> {
        match binding {
            Binding::Project { id, path, is_git } => {
                let project = self.project(id)?;
                Ok(project.path == *path && project.is_git == *is_git)
            }
            Binding::Source {
                project,
                id,
                path,
                is_git,
            } => {
                let source = self.source(project, id)?;
                Ok(source.path == *path && source.is_git == *is_git)
            }
            Binding::Directory {
                workspace,
                root,
                id,
                source,
                path,
                is_git,
            } => {
                let (workspace, directory) = self.directory(workspace, source)?;
                Ok(workspace.root == *root
                    && directory.id == *id
                    && directory.path == *path
                    && directory.is_git == *is_git)
            }
            Binding::External { project, id, path } => {
                Ok(self.external(project, id)?.path == *path)
            }
            Binding::Run {
                id,
                checkout,
                git_root,
            } => {
                let run = self.run(id)?;
                Ok(run.checkout == *checkout && run.git_root == *git_root)
            }
            Binding::Stage { run, stage } => Ok(self.run(run)?.stages.iter().any(|current| {
                current.stage_id == stage.stage_id
                    && current.start_sha == stage.start_sha
                    && current.completion_sha == stage.completion_sha
                    && current.publication == stage.publication
            })),
        }
    }
}
