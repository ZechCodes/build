use crate::app::config::configured_isolation;
use crate::app::{expand_tilde, AppState};
use crate::isolation::Isolation;
use crate::orchestrator::{ActiveRun, Orchestrator};
use crate::templates::Templates;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct ProjectSource {
    pub(in crate::app) id: String,
    pub(in crate::app) name: String,
    pub(in crate::app) mount: String,
    pub(in crate::app) path: std::path::PathBuf,
    pub(in crate::app) is_git: bool,
    pub(in crate::app) base_branch: String,
}

impl ProjectSource {
    /// The remote this source clones from and pushes to: its checkout's
    /// `origin`, read where git keeps it. The config holds no copy, so there
    /// is nothing to drift from the checkout.
    pub(in crate::app) fn origin(&self) -> Option<String> {
        self.is_git
            .then(|| crate::worktree::git_origin_url(&self.path))
            .flatten()
    }

    /// The source as every project row carries it.
    pub(in crate::app) fn wire(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "mount": self.mount,
            "path": self.path.display().to_string(),
            "is_git": self.is_git,
            "base_branch": self.base_branch,
            "remote": self.origin(),
        })
    }
}

mod agent_tools;
mod agent_writes;
pub(in crate::app) use agent_writes::{
    AgentChoiceArgs, ProjectSourceArgs, WorkspaceAgentAddress, WorkspaceDirectoryArgs,
};
mod conversation;
pub(in crate::app) use conversation::scratch_dir;
#[cfg(test)]
pub(in crate::app) use conversation::PROJECT_SCRATCH_DIR_NAME;
mod deletion;
mod lifecycle;
mod list;
pub(in crate::app) use list::primary_remote;
mod project_registry;
#[cfg(test)]
mod project_registry_tests;
mod requests;

use project_registry::ProjectCandidate;
pub(in crate::app) use project_registry::ProjectRegistry;

/// One registered project: a git repo, its base branch, and the orchestrator that
/// drives tasks on it. Each project gets its own worktrees subdir and orchestrator
/// so tasks on different repos never interact.
pub(in crate::app) struct Project {
    pub(in crate::app) id: String,
    pub(in crate::app) name: String,
    pub(in crate::app) repo_path: std::path::PathBuf,
    pub(in crate::app) base_branch: String,
    pub(in crate::app) is_git: bool,
    pub(in crate::app) sources: Vec<ProjectSource>,
    /// The number the next `source-N` is minted from. Held past every id this
    /// project has ever carried, not just the ones it still holds: a workspace
    /// records the id it was cut from, so handing a removed id to another
    /// folder would make one name mean two repositories.
    pub(in crate::app) next_source: u64,
    pub(in crate::app) orch: Orchestrator,
    /// Which isolation this project's new checkouts are made with, when the
    /// account's answer is not the one wanted here. `None` inherits it.
    pub(in crate::app) isolation: Option<Isolation>,
}

/// The default folder cloned repos land in, `~/.build/projects`.
pub(in crate::app) fn default_projects_dir() -> std::path::PathBuf {
    expand_tilde("~/.build/projects")
}

/// A project name that can be a directory: not empty, one path segment, and
/// nothing that climbs out of the folder it is going into.
pub(in crate::app) fn usable_project_name(name: impl AsRef<str>) -> Result<String, String> {
    let name = name.as_ref().trim();
    if name.is_empty() || name.contains('/') || name.contains("..") {
        return Err(format!("invalid project name: {name:?}"));
    }
    Ok(name.to_string())
}

/// The number past every `source-N` in this set — where a project that has
/// only ever appended starts minting.
pub(in crate::app) fn next_source_number(sources: &[ProjectSource]) -> u64 {
    sources
        .iter()
        .filter_map(|source| source.id.strip_prefix("source-"))
        .filter_map(|number| number.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

pub(in crate::app) fn safe_mount_name(name: &str) -> String {
    let mount = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let mount = mount.trim_matches(['.', '-']).to_string();
    if mount.is_empty() || mount == ".." {
        "source".to_string()
    } else {
        mount
    }
}

/// Resolve the exact directory selected as a source. Git classification is
/// intentionally left to the repository opener, which recognizes a `.git`
/// directory or indirection file at this path. A plain directory nested inside
/// some unrelated parent checkout remains its own source boundary.
pub(in crate::app) fn canonical_source_path(
    path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot open source {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "source is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

/// The base branch a project verb was told to use, if it was told one. With
/// none, the repository's own checked-out default answers — read off the disk
/// where the rest of the repository is read.
pub(in crate::app) fn requested_base_branch(params: &Value) -> Option<String> {
    params
        .get("base_branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
}

/// Derive a project folder name from a clone URL: the last path segment with a
/// trailing `.git` stripped (`git@host:org/repo.git` → `repo`).
pub(in crate::app) fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or("repo");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

impl AppState {
    #[cfg(test)]
    pub(in crate::app) fn project_at(&self, index: usize) -> &Project {
        self.projects.at(index)
    }

    #[cfg(test)]
    pub(in crate::app) fn project_at_mut(&mut self, index: usize) -> &mut Project {
        self.projects.at_mut(index)
    }

    pub(in crate::app) fn restore_configured_projects(&mut self, config: &Value) {
        if let Some(next) = config.get("next_project").and_then(Value::as_u64) {
            self.projects.reserve_ids_through(next);
        }
        let projects = config
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for project in projects {
            let stored_sources = project.get("sources").and_then(Value::as_array);
            let primary_path = project
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| stored_sources?.first()?.get("path")?.as_str());
            let Some(repo) = primary_path else {
                continue;
            };
            let base = project
                .get("base_branch")
                .and_then(Value::as_str)
                .unwrap_or("main")
                .to_string();
            let repo = std::path::PathBuf::from(repo);
            if !repo.exists() {
                continue;
            }
            let number = project
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.strip_prefix("proj-"))
                .and_then(|id| id.parse::<u64>().ok())
                .filter(|number| *number > 0 && *number < u64::MAX)
                .unwrap_or(self.projects.next_id());
            let repo = std::fs::canonicalize(&repo).unwrap_or(repo);
            let id = if let Some(existing) = self.projects.find_by_canonical_path(&repo) {
                existing.id.clone()
            } else {
                if self.projects.get(&format!("proj-{number}")).is_some() {
                    continue;
                }
                let candidate = self.project_candidate_at(
                    repo.clone(),
                    base,
                    repo.join(".git").exists(),
                    Some(number),
                );
                self.insert_project(candidate)
            };
            if let Some(entries) = stored_sources {
                let sources = entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, source)| {
                        let path = source.get("path")?.as_str()?;
                        let path = canonical_source_path(&expand_tilde(path)).ok()?;
                        let fallback_name = path.file_name()?.to_string_lossy().into_owned();
                        let name = source
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(&fallback_name)
                            .to_string();
                        let mount = source
                            .get("mount")
                            .and_then(Value::as_str)
                            .map(safe_mount_name)
                            .unwrap_or_else(|| safe_mount_name(&name));
                        let is_git = path.join(".git").exists();
                        Some(ProjectSource {
                            id: source
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("source-{}", index + 1)),
                            name,
                            mount,
                            path,
                            is_git,
                            base_branch: source
                                .get("base_branch")
                                .and_then(Value::as_str)
                                .unwrap_or("main")
                                .to_string(),
                        })
                    })
                    .collect::<Vec<_>>();
                if !sources.is_empty() {
                    self.projects.set_sources(&id, sources);
                }
            }
            if let Some(next) = project.get("next_source").and_then(Value::as_u64) {
                self.projects.reserve_source_ids_through(&id, next);
            }
            let isolation = configured_isolation(project, "project isolation");
            self.projects.set_isolation(&id, isolation);
        }
    }

    /// Override where cloned repos land and the browser starts (e.g. from an env).
    pub fn set_projects_dir(&mut self, dir: std::path::PathBuf) {
        self.projects_dir = dir;
    }

    /// Seed where cloned repos land when the persisted device settings do not
    /// contain a user choice. Call before [`AppState::with_config`].
    pub fn with_projects_dir_default(mut self, dir: std::path::PathBuf) -> Self {
        self.set_projects_dir(dir);
        self
    }

    /// The project repo path to stamp on a persisted record: the live project's
    /// canonical path if it is registered, else the retained record path (a
    /// parked repo-missing entity), else empty.
    pub(in crate::app) fn project_path_for(&self, entity_id: &str) -> String {
        self.projects.project_path_for(entity_id)
    }

    /// Register a project (repo + base branch) and return its id. Idempotent: a
    /// repo already registered (by canonical path) returns its existing id. Each
    /// project gets an isolated worktrees subdir keyed by id.
    pub fn add_project(&mut self, repo_path: std::path::PathBuf, base_branch: String) -> String {
        let repo_path = std::fs::canonicalize(&repo_path).unwrap_or(repo_path);
        if let Some(existing) = self.projects.find_by_canonical_path(&repo_path) {
            return existing.id.clone();
        }
        let is_git = repo_path.join(".git").exists();
        let project = self.project_candidate(repo_path, base_branch, is_git);
        self.insert_project(project)
    }

    fn project_candidate(
        &self,
        repo_path: std::path::PathBuf,
        base_branch: String,
        is_git: bool,
    ) -> ProjectCandidate {
        self.project_candidate_at(repo_path, base_branch, is_git, None)
    }

    fn project_candidate_at(
        &self,
        repo_path: std::path::PathBuf,
        base_branch: String,
        is_git: bool,
        restored_number: Option<u64>,
    ) -> ProjectCandidate {
        let name = repo_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        let sources = vec![ProjectSource {
            id: "source-1".to_string(),
            name: name.clone(),
            mount: safe_mount_name(&name),
            path: repo_path.clone(),
            is_git,
            base_branch: base_branch.clone(),
        }];
        self.project_candidate_with_sources_at(
            repo_path,
            base_branch,
            is_git,
            sources,
            restored_number,
        )
    }

    fn project_candidate_with_sources(
        &self,
        repo_path: std::path::PathBuf,
        base_branch: String,
        is_git: bool,
        sources: Vec<ProjectSource>,
    ) -> ProjectCandidate {
        self.project_candidate_with_sources_at(repo_path, base_branch, is_git, sources, None)
    }

    fn project_candidate_with_sources_at(
        &self,
        repo_path: std::path::PathBuf,
        base_branch: String,
        is_git: bool,
        sources: Vec<ProjectSource>,
        restored_number: Option<u64>,
    ) -> ProjectCandidate {
        let worktrees_root = self.worktrees_root.clone();
        let agent = self.agent.clone();
        let bridge_exe = self.bridge_exe.clone();
        self.projects.candidate_with_id(
            restored_number.unwrap_or(self.projects.next_id()),
            repo_path,
            base_branch,
            move |id, repo_path, base_branch| {
                let id = id.into_string();
                let name = repo_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                let orch = Orchestrator::new(
                    repo_path.to_path_buf(),
                    worktrees_root.join(&id),
                    agent,
                    Templates::default(),
                    bridge_exe,
                );
                Project {
                    id,
                    name,
                    repo_path: repo_path.to_path_buf(),
                    base_branch: base_branch.to_string(),
                    is_git,
                    next_source: next_source_number(&sources),
                    sources,
                    orch,
                    isolation: None,
                }
            },
        )
    }

    fn insert_project(&mut self, project: ProjectCandidate) -> String {
        let is_git = project.project().is_git;
        let id = self.projects.publish(project).into_string();
        if is_git {
            self.board.diff_mut().register_project(id.clone());
        }
        // A project is a section of the feed; registering one adds every row
        // its checkouts stand behind, and the project list a client holds has
        // a name on it that was not there.
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        id
    }

    #[cfg(test)]
    pub(in crate::app) fn clear_projects_for_test(&mut self) {
        let project_ids = self.projects.ids().map(str::to_string).collect::<Vec<_>>();
        self.projects.clear();
        for project_id in project_ids {
            self.board.diff_mut().remove_project(&project_id);
        }
    }

    /// The path of every Build-bound worktree — one per run: they are Build's,
    /// never external. As recorded, not canonicalized: this is read under the
    /// app mutex by every decide phase that hands a scan its exclusions, and
    /// the façade's `discover` canonicalizes them off the lock.
    pub(in crate::app) fn bound_worktree_paths(
        &self,
    ) -> std::collections::HashSet<std::path::PathBuf> {
        // Run worktrees are Build's. Tasks own no worktree at all — their
        // agents run in the primary checkout — so there is nothing to add here
        // for them.
        self.runs
            .values()
            .map(|active| active.worktree.path.clone())
            .collect()
    }

    /// Where one project's checkouts are materialized: its own directory under
    /// the bridge's worktrees root, the root its orchestrator was given.
    pub(in crate::app) fn project_worktrees_root(&self, project_id: &str) -> std::path::PathBuf {
        self.worktrees_root.join(project_id)
    }

    /// One registered project, to be read. The shared-borrow half of
    /// [`Self::project_mut`]: every read of a project's caches or repository
    /// resolves it through here.
    pub(in crate::app) fn project(&self, project_id: &str) -> Option<&Project> {
        self.projects.get(project_id)
    }

    pub(in crate::app) fn sources_for(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectSource>, String> {
        Ok(self.project_for(project_id)?.sources.clone())
    }

    /// The registered project a client names by id, or the one refusal every
    /// verb that takes a `project_id` gives when nothing is registered under it.
    pub(in crate::app) fn project_for(&self, project_id: &str) -> Result<&Project, String> {
        self.projects
            .get(project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))
    }

    /// The orchestrator for a project id.
    pub(in crate::app) fn orch_for(&self, project_id: &str) -> Result<&Orchestrator, String> {
        let project = self.project_for(project_id)?;
        if !project.is_git {
            return Err("project is not a git repository; initialize Git first".to_string());
        }
        Ok(&project.orch)
    }

    /// The base branch configured for a project id.
    pub(in crate::app) fn base_for(&self, project_id: &str) -> Result<String, String> {
        Ok(self.project_for(project_id)?.base_branch.clone())
    }

    /// A project's repository root — the directory the project's sources are
    /// read from, and the same one `TermScope::Primary` resolves to.
    pub(in crate::app) fn repo_path_for(
        &self,
        project_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        Ok(self.project_for(project_id)?.repo_path.clone())
    }

    /// Whether a run stands in its project's repository rather than in a
    /// checkout cut beside it.
    ///
    /// Nothing mints such a run any more — work happens in workspaces — but a
    /// store written before that holds runs adopted on the repo root, and the
    /// verbs that remove a run's directory must never remove the repository.
    /// Compared by canonical path, off the two paths the run record already
    /// carries, so the guard holds for a run this daemon never minted and can
    /// never disagree with where the run actually works.
    pub(in crate::app) fn stands_in_the_repository(
        &self,
        run_id: &str,
        active: &ActiveRun,
    ) -> bool {
        let repo_path = self.project_path_for(run_id);
        !repo_path.is_empty()
            && Self::canonical_root(std::path::Path::new(&repo_path))
                == Self::canonical_root(&active.worktree.path)
    }

    /// The project an entity (plan or run) belongs to.
    pub(in crate::app) fn project_of(&self, entity_id: &str) -> Result<String, String> {
        self.projects
            .project_id_of(entity_id)
            .map(str::to_string)
            .ok_or_else(|| "unknown entity id".to_string())
    }

    /// The default project id when the client doesn't choose one (the first
    /// registered project).
    pub(in crate::app) fn default_project(&self) -> Result<String, String> {
        self.projects
            .iter()
            .next()
            .map(|p| p.id.clone())
            .ok_or_else(|| "no projects configured".to_string())
    }

    pub(in crate::app) fn project_name_of(&self, entity_id: &str) -> String {
        let Some(project_id) = self.projects.project_id_of(entity_id) else {
            return String::new();
        };
        self.project_name_by_id(project_id)
    }

    /// What a project is called, or "" when this bridge has no such project —
    /// a row names a project it cannot resolve rather than failing to exist.
    pub(in crate::app) fn project_name_by_id(&self, project_id: &str) -> String {
        self.projects
            .get(project_id)
            .map(|project| project.name.clone())
            .unwrap_or_default()
    }
}
