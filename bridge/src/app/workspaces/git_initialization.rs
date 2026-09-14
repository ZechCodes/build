use super::workspace_json;
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{require_str, AppState, DeferredGit, DeferredWork};
use crate::workspace::{DirectoryStatus, Workspace, WorkspaceDirectory, WorkspaceStatus};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum GitInitTarget {
    Workspace,
    Source,
}

impl GitInitTarget {
    fn wire(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Source => "source",
        }
    }
}

#[derive(Clone)]
struct GitInitLocation {
    target: GitInitTarget,
    path: PathBuf,
    base_branch: String,
    identity: DirectoryIdentity,
}

#[derive(Clone)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl DirectoryIdentity {
    fn capture(path: &Path) -> Result<Self, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("inspect git init target {}: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Ok(Self {})
        }
    }

    fn verify(&self, path: &Path) -> Result<(), String> {
        let current = Self::capture(path)?;
        #[cfg(unix)]
        if current.device != self.device || current.inode != self.inode {
            return Err("git init target was replaced after it was selected".to_string());
        }
        Ok(())
    }
}

struct WorkspaceGitInitWork {
    workspace: Workspace,
    directory: WorkspaceDirectory,
    locations: Vec<(GitInitTarget, Result<GitInitLocation, String>)>,
}

impl DeferredGitWork for WorkspaceGitInitWork {
    fn run(&self, _params: &Value) -> Result<Value, String> {
        let mut initialized_paths = HashMap::<PathBuf, (String, bool)>::new();
        let results = self
            .locations
            .iter()
            .map(|(target, location)| {
                run_git_init_target(*target, location, &mut initialized_paths)
            })
            .collect::<Vec<_>>();
        Ok(json!({"results": results}))
    }

    fn invalidate(&self, _app: &mut AppState) {}

    fn settle(&self, app: &mut AppState, mut result: Value) -> Result<Value, String> {
        let outcomes = result["results"]
            .as_array_mut()
            .ok_or_else(|| "git initialization returned invalid outcomes".to_string())?;
        for (outcome, (_, location)) in outcomes.iter_mut().zip(&self.locations) {
            settle_git_init_target(app, &self.workspace, &self.directory, location, outcome);
        }
        result["workspace"] = app
            .workspaces
            .get(&self.workspace.id)
            .map(workspace_json)
            .unwrap_or(Value::Null);
        result["source"] = app
            .project_source_json(&self.workspace.project_id, &self.directory.source_id)
            .unwrap_or(Value::Null);
        Ok(result)
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
}

impl AppState {
    pub(crate) fn workspace_git_init_options(&mut self, params: &Value) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        let source_id = require_str(params, "source_id")?;
        let directory = self.resolve_workspace_directory(&workspace_id, &source_id)?;
        let source_id = directory.source_id.clone();
        let workspace = self
            .workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        let workspace_option = self
            .workspace_init_location(&workspace, &directory)
            .map(|location| git_init_option(&location.path, directory.is_git))
            .unwrap_or_else(
                |reason| json!({"available": false, "is_git": false, "reason": reason}),
            );
        let source_option = self
            .source_init_location(&workspace, &directory)
            .map(|location| {
                let recorded = self
                    .projects
                    .get(&workspace.project_id)
                    .and_then(|project| {
                        project.sources.iter().find(|source| source.id == source_id)
                    })
                    .is_some_and(|source| source.is_git);
                git_init_option(&location.path, recorded)
            })
            .unwrap_or_else(
                |reason| json!({"available": false, "is_git": false, "reason": reason}),
            );
        Ok(json!({
            "workspace_id": workspace_id,
            "source_id": source_id,
            "workspace": workspace_option,
            "source": source_option,
        }))
    }

    pub(crate) fn workspace_init_git(&mut self, params: &Value) -> Result<Value, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        let source_id = require_str(params, "source_id")?;
        let targets = parse_git_init_targets(require_str(params, "target")?.as_str())?;
        let directory = self.resolve_workspace_directory(&workspace_id, &source_id)?;
        let workspace = self
            .workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        let mut locations = Vec::with_capacity(targets.len());
        for target in targets {
            let location = match target {
                GitInitTarget::Workspace => self.workspace_init_location(&workspace, &directory),
                GitInitTarget::Source => self.source_init_location(&workspace, &directory),
            };
            locations.push((target, location));
        }

        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(WorkspaceGitInitWork {
                workspace,
                directory,
                locations,
            }),
            params: json!({"workspace_id": workspace_id}),
            invalidates: true,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Ok(Value::Null)
    }

    fn workspace_init_location(
        &self,
        workspace: &Workspace,
        directory: &WorkspaceDirectory,
    ) -> Result<GitInitLocation, String> {
        if workspace.status != WorkspaceStatus::Ready || directory.status != DirectoryStatus::Ready
        {
            return Err("workspace directory is not ready for Git initialization".to_string());
        }
        let path = std::fs::canonicalize(&directory.path).map_err(|error| {
            format!(
                "resolve workspace directory {}: {error}",
                directory.path.display()
            )
        })?;
        if path != directory.path || !path.is_dir() {
            return Err("workspace target is not a directory".to_string());
        }
        let root = std::fs::canonicalize(&workspace.root).map_err(|error| {
            format!(
                "resolve workspace root {}: {error}",
                workspace.root.display()
            )
        })?;
        let valid = if workspace.managed {
            path.parent() == Some(root.as_path())
        } else {
            path == root
        };
        if !valid {
            return Err("workspace directory is outside its recorded root".to_string());
        }
        let base_branch = if !workspace.managed && directory.base_branch.is_empty() {
            self.projects
                .get(&workspace.project_id)
                .and_then(|project| {
                    project
                        .sources
                        .iter()
                        .find(|source| source.id == directory.source_id)
                })
                .map(|source| source.base_branch.clone())
                .ok_or_else(|| "adopted workspace has no matching project source".to_string())?
        } else {
            directory.base_branch.clone()
        };
        let identity = DirectoryIdentity::capture(&path)?;
        Ok(GitInitLocation {
            target: GitInitTarget::Workspace,
            path,
            base_branch,
            identity,
        })
    }

    fn source_init_location(
        &self,
        workspace: &Workspace,
        directory: &WorkspaceDirectory,
    ) -> Result<GitInitLocation, String> {
        let project = self
            .projects
            .get(&workspace.project_id)
            .ok_or_else(|| format!("unknown project: {}", workspace.project_id))?;
        let source = project
            .sources
            .iter()
            .find(|source| source.id == directory.source_id)
            .ok_or_else(|| {
                format!(
                    "unknown source_id {} in project {}",
                    directory.source_id, workspace.project_id
                )
            })?;
        let path = std::fs::canonicalize(&source.path).map_err(|error| {
            format!("resolve project source {}: {error}", source.path.display())
        })?;
        let recorded = if !workspace.managed
            && directory.source_path.as_os_str().is_empty()
            && directory.path == workspace.root
        {
            path.clone()
        } else {
            std::fs::canonicalize(&directory.source_path).map_err(|error| {
                format!(
                    "resolve recorded source {}: {error}",
                    directory.source_path.display()
                )
            })?
        };
        if path != source.path
            || (!directory.source_path.as_os_str().is_empty() && recorded != directory.source_path)
            || path != recorded
            || !path.is_dir()
        {
            return Err("project source no longer matches the workspace record".to_string());
        }
        let identity = DirectoryIdentity::capture(&path)?;
        Ok(GitInitLocation {
            target: GitInitTarget::Source,
            path,
            base_branch: source.base_branch.clone(),
            identity,
        })
    }

    fn record_git_target(
        &mut self,
        workspace: &Workspace,
        directory: &WorkspaceDirectory,
        location: &GitInitLocation,
        branch: &str,
    ) -> Result<(), String> {
        match location.target {
            GitInitTarget::Workspace => {
                self.workspaces.record_git_repository(
                    &workspace.id,
                    &directory.source_id,
                    branch,
                )?;
            }
            GitInitTarget::Source => {
                let primary = self.projects.mark_source_git(
                    &workspace.project_id,
                    &directory.source_id,
                    &location.path,
                    branch,
                )?;
                if primary {
                    self.board
                        .diff_mut()
                        .register_project(workspace.project_id.clone());
                }
                let config =
                    self.config_value(&self.projects_dir, self.default_harness, self.isolation);
                self.persist_config(&config)?;
            }
        }
        Ok(())
    }

    fn project_source_json(&self, project_id: &str, source_id: &str) -> Result<Value, String> {
        let project = self
            .projects
            .get(project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        let source = project
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        Ok(json!({"id": source.id, "is_git": source.is_git, "base_branch": source.base_branch}))
    }
}

fn run_git_init_target(
    target: GitInitTarget,
    location: &Result<GitInitLocation, String>,
    initialized_paths: &mut HashMap<PathBuf, (String, bool)>,
) -> Value {
    let location = match location {
        Ok(location) => location,
        Err(error) => return git_init_failure(target, error.clone()),
    };
    if let Err(error) = location.identity.verify(&location.path) {
        return git_init_failure(target, error);
    }
    let repository = if let Some((head_branch, _)) = initialized_paths.get(&location.path) {
        effective_base_for(&location.path, &location.base_branch, head_branch)
            .map(|effective_base| (head_branch.clone(), effective_base, false))
    } else {
        initialize_exact_repository(&location.path, &location.base_branch).inspect(|outcome| {
            initialized_paths.insert(location.path.clone(), (outcome.0.clone(), outcome.2));
        })
    };
    match repository {
        Err(error) => git_init_failure(target, error),
        Ok((head_branch, effective_base, was_new)) => json!({
            "target": target.wire(),
            "status": if was_new { "initialized" } else { "already_initialized" },
            "is_git": true,
            "_branch": if target == GitInitTarget::Workspace { head_branch.clone() } else { effective_base.clone() },
            "_head_branch": head_branch,
            "_effective_base": effective_base,
        }),
    }
}

fn effective_base_for(path: &Path, configured: &str, head_branch: &str) -> Result<String, String> {
    let repository = git2::Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .map_err(|error| format!("open existing repository: {error}"))?;
    Ok(repository
        .revparse_single(configured)
        .ok()
        .and_then(|object| object.peel_to_commit().ok())
        .map_or_else(|| head_branch.to_string(), |_| configured.to_string()))
}

fn settle_git_init_target(
    app: &mut AppState,
    workspace: &Workspace,
    directory: &WorkspaceDirectory,
    location: &Result<GitInitLocation, String>,
    outcome: &mut Value,
) {
    let Some(branch) = outcome
        .get("_branch")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    let head_branch = outcome
        .get("_head_branch")
        .and_then(Value::as_str)
        .unwrap_or(&branch)
        .to_string();
    let result = location
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|location| {
            location.identity.verify(&location.path)?;
            if location.target == GitInitTarget::Source {
                let validated = app.source_init_location(workspace, directory)?;
                if validated.path != location.path {
                    return Err("Git source changed while it was initialized".to_string());
                }
                app.record_git_target(workspace, directory, &validated, &branch)?;
                if let Ok((current_workspace, current, copy)) =
                    current_workspace_location(app, workspace, directory)
                {
                    if copy.path == location.path {
                        app.workspaces.record_git_repository(
                            &current_workspace.id,
                            &current.source_id,
                            &head_branch,
                        )?;
                    }
                }
                return Ok(());
            }
            let (current_workspace, current, validated) =
                current_workspace_location(app, workspace, directory)?;
            if validated.path != location.path {
                return Err("Git target changed while it was initialized".to_string());
            }
            app.record_git_target(&current_workspace, &current, &validated, &branch)?;
            if let Ok(source) = app.source_init_location(&current_workspace, &current) {
                if source.path == validated.path {
                    let source_base =
                        effective_base_for(&source.path, &source.base_branch, &head_branch)?;
                    app.record_git_target(&current_workspace, &current, &source, &source_base)?;
                }
            }
            Ok(())
        });
    outcome
        .as_object_mut()
        .expect("git outcome object")
        .remove("_branch");
    outcome
        .as_object_mut()
        .expect("git outcome object")
        .remove("_head_branch");
    outcome
        .as_object_mut()
        .expect("git outcome object")
        .remove("_effective_base");
    if let Err(error) = result {
        outcome["status"] = json!("failed");
        outcome["error"] = json!(error);
    }
}

fn current_workspace_location(
    app: &AppState,
    workspace: &Workspace,
    directory: &WorkspaceDirectory,
) -> Result<(Workspace, WorkspaceDirectory, GitInitLocation), String> {
    let current_workspace = app
        .workspaces
        .get(&workspace.id)
        .cloned()
        .ok_or_else(|| "workspace was removed while Git was initialized".to_string())?;
    if current_workspace.project_id != workspace.project_id
        || current_workspace.root != workspace.root
        || current_workspace.managed != workspace.managed
    {
        return Err("workspace identity changed while Git was initialized".to_string());
    }
    let current = current_workspace
        .directories
        .iter()
        .find(|current| current.source_id == directory.source_id)
        .cloned()
        .ok_or_else(|| "workspace source was removed while Git was initialized".to_string())?;
    if current.id != directory.id
        || current.path != directory.path
        || current.source_path != directory.source_path
    {
        return Err("workspace directory changed while Git was initialized".to_string());
    }
    let validated = app.workspace_init_location(&current_workspace, &current)?;
    Ok((current_workspace, current, validated))
}

fn parse_git_init_targets(target: &str) -> Result<Vec<GitInitTarget>, String> {
    match target {
        "workspace" => Ok(vec![GitInitTarget::Workspace]),
        "source" => Ok(vec![GitInitTarget::Source]),
        "both" => Ok(vec![GitInitTarget::Workspace, GitInitTarget::Source]),
        value => Err(format!("unknown git init target: {value}")),
    }
}

fn git_init_option(path: &Path, recorded_is_git: bool) -> Value {
    match inspect_exact_repository(path) {
        Ok(Some((_, has_commit))) => json!({
            "path": path.display().to_string(),
            "available": true,
            "is_git": has_commit,
            "needs_reconciliation": !recorded_is_git || !has_commit,
        }),
        Ok(None) => match std::fs::symlink_metadata(path.join(".git")) {
            Ok(_) => {
                json!({"path": path.display().to_string(), "available": false, "is_git": false, "reason": "an invalid .git marker already exists"})
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                json!({"path": path.display().to_string(), "available": true, "is_git": false})
            }
            Err(error) => {
                json!({"path": path.display().to_string(), "available": false, "is_git": false, "reason": format!("inspect .git marker: {error}")})
            }
        },
        Err(error) => {
            json!({"path": path.display().to_string(), "available": false, "is_git": false, "reason": error})
        }
    }
}

fn git_init_failure(target: GitInitTarget, error: String) -> Value {
    json!({"target": target.wire(), "status": "failed", "is_git": false, "error": error})
}

fn initialize_exact_repository(
    path: &Path,
    base_branch: &str,
) -> Result<(String, String, bool), String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("resolve git init target {}: {error}", path.display()))?;
    if canonical != path || !canonical.is_dir() {
        return Err("git init target changed after it was selected".to_string());
    }
    let reference = format!("refs/heads/{base_branch}");
    if base_branch.trim().is_empty()
        || base_branch.contains('\0')
        || !git2::Reference::is_valid_name(&reference)
    {
        return Err(format!("invalid initial branch: {base_branch:?}"));
    }
    if let Some((branch, has_commit)) = inspect_exact_repository(path)? {
        if has_commit {
            let effective_base = effective_base_for(path, base_branch, &branch)?;
            return Ok((branch, effective_base, false));
        }
        create_empty_initial_commit(path)?;
        return Ok((branch.clone(), branch, true));
    }
    match std::fs::symlink_metadata(path.join(".git")) {
        Ok(_) => return Err("refusing to replace an existing invalid .git marker".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("inspect .git marker: {error}")),
    }
    let mut options = git2::RepositoryInitOptions::new();
    options
        .initial_head(base_branch)
        .external_template(false)
        .mkdir(false)
        .mkpath(false)
        .no_reinit(true);
    git2::Repository::init_opts(path, &options)
        .map_err(|error| format!("git init failed: {error}"))?;
    create_empty_initial_commit(path)?;
    Ok((base_branch.to_string(), base_branch.to_string(), true))
}

fn create_empty_initial_commit(path: &Path) -> Result<(), String> {
    let repo = git2::Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    )
    .map_err(|error| format!("open initialized repository: {error}"))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| "initialized repository is bare".to_string())?;
    if std::fs::canonicalize(workdir).ok().as_deref() != Some(path) {
        return Err("initialized repository worktree changed".to_string());
    }
    let tree_id = repo
        .treebuilder(None)
        .and_then(|tree| tree.write())
        .map_err(|error| format!("cannot create empty git tree: {error}"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(|error| format!("cannot read empty git tree: {error}"))?;
    let signature = git2::Signature::now("Build", "build@build.ing")
        .map_err(|error| format!("cannot create git identity: {error}"))?;
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Initial commit",
        &tree,
        &[],
    )
    .map_err(|error| format!("cannot create initial git commit: {error}"))?;
    Ok(())
}

fn inspect_exact_repository(path: &Path) -> Result<Option<(String, bool)>, String> {
    if std::fs::symlink_metadata(path.join(".git"))
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("refusing a symlink .git marker".to_string());
    }
    let repository = match git2::Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&Path>(),
    ) {
        Ok(repository) => repository,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect repository: {error}")),
    };
    let workdir = repository
        .workdir()
        .ok_or_else(|| "bare repositories cannot be initialized as workspaces".to_string())?;
    let workdir = std::fs::canonicalize(workdir)
        .map_err(|error| format!("resolve repository worktree: {error}"))?;
    if workdir != path {
        return Err("repository worktree does not match the selected directory".to_string());
    }
    let result = match repository.head() {
        Ok(head) if head.is_branch() => Ok(Some((
            head.shorthand()
                .ok_or_else(|| "repository branch has no name".to_string())?
                .to_string(),
            true,
        ))),
        Ok(_) => Err("repository HEAD is detached".to_string()),
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => {
            let head = repository
                .find_reference("HEAD")
                .map_err(|error| format!("read unborn repository HEAD: {error}"))?;
            let branch = head
                .symbolic_target()
                .and_then(|target| target.strip_prefix("refs/heads/"))
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "unborn repository HEAD has no branch".to_string())?;
            Ok(Some((branch.to_string(), false)))
        }
        Err(error) => Err(format!("repository has no usable HEAD: {error}")),
    };
    result
}
