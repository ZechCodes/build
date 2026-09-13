use super::{
    canonical_source_path, repo_name_from_url, requested_base_branch, safe_mount_name,
    usable_project_name, ProjectSource,
};
use crate::app::config::accept_isolation;
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{
    CloneRepo, CreateRepo, InitializeRepo, OpenRepo, PendingRow, PendingState, Performed,
    SetRemote, WorktreeChange, WorktreeMutation,
};
use crate::worktree::git_remote_origin;
use serde_json::Value;

struct SourceRequest {
    path: Option<std::path::PathBuf>,
    remote: Option<String>,
    name: String,
    mount: String,
    base_branch: Option<String>,
}

struct PreparedSources {
    opened: crate::lifecycle::OpenedRepository,
    sources: Vec<ProjectSource>,
    created_checkouts: Vec<std::path::PathBuf>,
}

struct OpenProjectSources {
    requests: Vec<SourceRequest>,
    managed_root: std::path::PathBuf,
}

impl WorktreeMutation for OpenProjectSources {
    type Output = PreparedSources;

    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let mut sources = Vec::with_capacity(self.requests.len());
        let mut primary = None;
        let mut created_checkouts = Vec::new();
        for (index, request) in self.requests.into_iter().enumerate() {
            let result = if let Some(path) = request.path {
                canonical_source_path(&path).and_then(|path| {
                    OpenRepo {
                        path,
                        requested_base: request.base_branch,
                    }
                    .perform()
                })
            } else {
                let remote = request.remote.clone().expect("validated source remote");
                let dest = self.managed_root.join(&request.mount);
                CloneRepo {
                    url: remote,
                    name: request.mount.clone(),
                    dest,
                    projects_dir: self.managed_root.clone(),
                    requested_base: request.base_branch,
                }
                .perform()
            };
            let performed = match result {
                Ok(performed) => performed,
                Err(error) => {
                    for path in created_checkouts {
                        let _ = std::fs::remove_dir_all(path);
                    }
                    return Err(error);
                }
            };
            let opened = performed.output;
            if let Some(path) = &opened.created_checkout {
                created_checkouts.push(path.clone());
            }
            sources.push(ProjectSource {
                id: format!("source-{}", index + 1),
                name: request.name,
                mount: request.mount,
                path: opened.path.clone(),
                is_git: opened.is_git,
                base_branch: opened.base.clone(),
                remote: opened.remote.clone().or(request.remote),
            });
            if primary.is_none() {
                primary = Some(opened);
            }
        }
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: PreparedSources {
                opened: primary.expect("non-empty sources"),
                sources,
                created_checkouts,
            },
        })
    }
}

impl AppState {
    /// Register a project from a host path. Validates it is a git repo with the
    /// requested base branch before adding, so a bad path fails loudly here rather
    /// than at first dispatch.
    pub(in crate::app) fn project_add(&mut self, params: &Value) -> Result<Value, String> {
        if params.get("sources").is_some() {
            return self.project_from_sources(params, None);
        }
        let path = require_str(params, "path")?;
        let path = expand_tilde(&path);
        self.defer_project(
            path.clone(),
            path.display().to_string(),
            PendingState::Creating,
            OpenRepo {
                requested_base: requested_base_branch(params),
                path,
            },
            crate::app::runtime::lifecycle::ProjectRegistrationSettlement,
        )
    }

    pub(in crate::app) fn project_init_git(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .get(&project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        if project.is_git {
            return Err("project is already a git repository".to_string());
        }
        let path = project.repo_path.clone();
        let base_branch = project.base_branch.clone();
        let title = project.name.clone();
        self.defer_project(
            path.clone(),
            title,
            PendingState::Updating,
            InitializeRepo {
                project_id: project_id.clone(),
                path,
                base_branch,
            },
            crate::app::runtime::lifecycle::InitializeRepositorySettlement,
        )
    }

    /// Reserve the directory a project verb is about to read or write and hand
    /// its git to the drain. The directory is the row's identity: there is no
    /// project id until the git lands, and what two project verbs collide over
    /// is the folder, not a name.
    fn defer_project<T, S>(
        &mut self,
        dest: std::path::PathBuf,
        title: String,
        state: PendingState,
        mutation: T,
        settlement: S,
    ) -> Result<Value, String>
    where
        T: WorktreeMutation,
        S: crate::app::runtime::lifecycle::LifecycleSettlement<T::Output>,
    {
        let row = PendingRow::on_directory(
            crate::worktree::external_worktree_id(&crate::worktree::canonical_planned_path(&dest)),
            title,
            state,
        );
        self.defer_lifecycle(row, mutation, settlement)
    }

    /// Clone a remote into the projects folder and register it as a project. The
    /// base branch defaults to the clone's checked-out branch.
    pub(in crate::app) fn project_clone(&mut self, params: &Value) -> Result<Value, String> {
        let url = require_str(params, "url")?;
        let name = match params
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(named) => named.to_string(),
            None => repo_name_from_url(&url),
        };
        let name = usable_project_name(name)?;
        let dest = self.projects_dir.join(&name);
        self.defer_project(
            dest.clone(),
            name.clone(),
            PendingState::Creating,
            CloneRepo {
                url,
                name,
                dest,
                projects_dir: self.projects_dir.clone(),
                requested_base: requested_base_branch(params),
            },
            crate::app::runtime::lifecycle::ProjectRegistrationSettlement,
        )
    }

    /// Create a brand-new git repo (with an initial commit so its base branch
    /// resolves and tasks can dispatch) inside `parent` — a browsed-to directory,
    /// or the projects folder by default — and register it. An optional `remote`
    /// is wired as `origin` at creation.
    pub(in crate::app) fn project_create(&mut self, params: &Value) -> Result<Value, String> {
        let name = usable_project_name(require_str(params, "name")?)?;
        if params.get("sources").is_some() {
            return self.project_from_sources(params, Some(name));
        }
        let base_branch = params
            .get("base_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .unwrap_or("main")
            .to_string();
        let parent = match params
            .get("parent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|parent| !parent.is_empty())
        {
            Some(parent) => expand_tilde(parent),
            None => self.projects_dir.clone(),
        };
        let dest = parent.join(&name);
        self.defer_project(
            dest.clone(),
            name.clone(),
            PendingState::Creating,
            CreateRepo {
                name,
                dest,
                base_branch,
                remote: params
                    .get("remote")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|remote| !remote.is_empty())
                    .map(str::to_string),
            },
            crate::app::runtime::lifecycle::ProjectRegistrationSettlement,
        )
    }

    fn project_from_sources(
        &mut self,
        params: &Value,
        explicit_name: Option<String>,
    ) -> Result<Value, String> {
        let entries = params
            .get("sources")
            .and_then(Value::as_array)
            .ok_or_else(|| "sources must be an array".to_string())?;
        if entries.is_empty() {
            return Err("a project must have at least one source".to_string());
        }
        let mut requests = Vec::with_capacity(entries.len());
        let mut paths = Vec::new();
        let mut remotes = std::collections::HashSet::new();
        let mut mounts = std::collections::HashSet::new();
        for entry in entries {
            let path_text = entry
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let remote = entry
                .get("remote")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if path_text.is_some() == remote.is_some() {
                return Err("each source must specify exactly one of path or remote".to_string());
            }
            let path = path_text.map(expand_tilde);
            let canonical = path.as_deref().map(canonical_source_path).transpose()?;
            if let Some(candidate) = &canonical {
                if self
                    .projects
                    .iter()
                    .flat_map(|project| &project.sources)
                    .any(|existing| {
                        candidate.starts_with(&existing.path)
                            || existing.path.starts_with(candidate)
                    })
                {
                    return Err(format!(
                        "source overlaps a registered project source: {}",
                        candidate.display()
                    ));
                }
                if paths.iter().any(|existing: &std::path::PathBuf| {
                    candidate.starts_with(existing) || existing.starts_with(candidate)
                }) {
                    return Err(format!(
                        "source overlaps another source: {}",
                        candidate.display()
                    ));
                }
                paths.push(candidate.clone());
            }
            if let Some(remote) = remote {
                if self
                    .projects
                    .iter()
                    .flat_map(|project| &project.sources)
                    .any(|source| source.remote.as_deref() == Some(remote))
                {
                    return Err(format!("source remote is already registered: {remote}"));
                }
                if !remotes.insert(remote.to_string()) {
                    return Err(format!("duplicate source remote: {remote}"));
                }
            }
            let inferred = canonical
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .or_else(|| remote.map(repo_name_from_url))
                .unwrap_or_else(|| "source".to_string());
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(&inferred)
                .to_string();
            let base_mount = safe_mount_name(&name);
            let mut mount = base_mount.clone();
            let mut suffix = 2;
            while !mounts.insert(mount.clone()) {
                mount = format!("{base_mount}-{suffix}");
                suffix += 1;
            }
            requests.push(SourceRequest {
                path: canonical,
                remote: remote.map(str::to_string),
                name,
                mount,
                base_branch: entry
                    .get("base_branch")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            });
        }
        let project_name = explicit_name
            .or_else(|| {
                params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| requests[0].name.clone());
        let project_name = usable_project_name(safe_mount_name(&project_name))?;
        let managed_root = self.projects_dir.join(format!("{project_name}-sources"));
        let canonical_projects_dir =
            std::fs::canonicalize(&self.projects_dir).unwrap_or_else(|_| self.projects_dir.clone());
        let planned_managed_root = canonical_projects_dir.join(format!("{project_name}-sources"));
        for request in &requests {
            if request.remote.is_none() {
                continue;
            }
            let destination = planned_managed_root.join(&request.mount);
            if paths
                .iter()
                .any(|source| destination.starts_with(source) || source.starts_with(&destination))
            {
                return Err(format!(
                    "managed source destination overlaps a local source: {}",
                    destination.display()
                ));
            }
        }
        let title = project_name;
        self.defer_project(
            managed_root.clone(),
            title,
            PendingState::Creating,
            OpenProjectSources {
                requests,
                managed_root,
            },
            |state: &mut AppState, result: Result<PreparedSources, String>| {
                let prepared = result?;
                state.register_opened_project_sources(
                    prepared.opened,
                    prepared.sources,
                    prepared.created_checkouts,
                )
            },
        )
    }

    /// Set (or clear, with an empty url) a project's `origin` remote.
    pub(in crate::app) fn project_set_remote(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let url = require_str(params, "url")?;
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        if !project.is_git {
            return Err("project is not a git repository; initialize Git first".to_string());
        }
        let repo_path = project.repo_path.clone();
        let title = project.name.clone();
        // The repository is the row's identity here as it is for every other
        // project verb: what a second `set_remote` collides with is the config
        // file it would be rewriting, and nothing about the project's record
        // is being minted or taken away.
        self.defer_project(
            repo_path.clone(),
            title,
            PendingState::Updating,
            SetRemote {
                repo_path,
                url: url.trim().to_string(),
            },
            crate::app::runtime::lifecycle::SetRemoteSettlement { project_id },
        )
    }

    /// Set (or clear, with a null isolation) a project's override of the
    /// account's isolation. The choice is put to this project's volume before
    /// it is stored, so a client only ever repaints from a row the bridge would
    /// honour; naming no isolation at all is a missing param, not a clear.
    pub(in crate::app) fn project_set_isolation(
        &mut self,
        params: &Value,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .get(&project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        let isolation = match params.get("isolation") {
            Some(Value::Null) => None,
            _ => Some(accept_isolation(
                &require_str(params, "isolation")?,
                &project.orch.worktrees().availability(),
            )?),
        };
        assert!(
            self.projects.set_isolation(&project_id, isolation),
            "the project was just resolved"
        );
        self.persist();
        let project = self
            .projects
            .get(&project_id)
            .expect("the project was just resolved");
        let remote = git_remote_origin(&project.repo_path);
        Ok(self.project_json(project, remote))
    }
}
