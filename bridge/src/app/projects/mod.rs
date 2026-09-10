use crate::app::config::configured_isolation;
use crate::app::{expand_tilde, AppState};
use crate::isolation::Isolation;
use crate::orchestrator::{ActiveRun, Orchestrator};
use crate::templates::Templates;
use serde_json::Value;

mod lifecycle;
mod list;
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
        let projects = config
            .get("projects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for project in projects {
            let Some(repo) = project.get("path").and_then(Value::as_str) else {
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
            let id = self.add_project(repo, base);
            let isolation = configured_isolation(project, "project isolation");
            self.projects.set_isolation(&id, isolation);
        }
    }

    /// Override where cloned repos land and the browser starts (e.g. from an env).
    pub fn set_projects_dir(&mut self, dir: std::path::PathBuf) {
        self.projects_dir = dir;
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
        let project = self.project_candidate(repo_path, base_branch);
        self.insert_project(project)
    }

    fn project_candidate(
        &self,
        repo_path: std::path::PathBuf,
        base_branch: String,
    ) -> ProjectCandidate {
        let worktrees_root = self.worktrees_root.clone();
        let agent = self.agent.clone();
        let bridge_exe = self.bridge_exe.clone();
        self.projects
            .candidate(repo_path, base_branch, move |id, repo_path, base_branch| {
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
                    orch,
                    isolation: None,
                }
            })
    }

    fn insert_project(&mut self, project: ProjectCandidate) -> String {
        let id = self.projects.publish(project).into_string();
        self.board.diff_mut().register_project(id.clone());
        // A project is a section of the feed; registering one adds every row
        // its checkouts stand behind.
        self.note_board_changed();
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
        // Run worktrees are Build's. Issues own no worktree at all — their
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

    /// The registered project a client names by id, or the one refusal every
    /// verb that takes a `project_id` gives when nothing is registered under it.
    pub(in crate::app) fn project_for(&self, project_id: &str) -> Result<&Project, String> {
        self.projects
            .get(project_id)
            .ok_or_else(|| format!("unknown project_id: {project_id}"))
    }

    /// The orchestrator for a project id.
    pub(in crate::app) fn orch_for(&self, project_id: &str) -> Result<&Orchestrator, String> {
        Ok(&self.project_for(project_id)?.orch)
    }

    /// The base branch configured for a project id.
    pub(in crate::app) fn base_for(&self, project_id: &str) -> Result<String, String> {
        Ok(self.project_for(project_id)?.base_branch.clone())
    }

    /// A project's primary checkout — the repo root, the same directory
    /// `TermScope::Primary` resolves to.
    pub(in crate::app) fn repo_path_for(
        &self,
        project_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        Ok(self.project_for(project_id)?.repo_path.clone())
    }

    /// Whether a run was adopted around its project's primary checkout rather
    /// than a worktree beside it.
    ///
    /// Derived from the two paths the run record already carries (its worktree
    /// and its project), so it survives a daemon restart with no new stored
    /// field and can never disagree with where the run actually works. Takes
    /// the run by reference because the callers that matter most — `run_view`
    /// and the lifecycle guards — hold it outside the map.
    pub(in crate::app) fn owns_primary_checkout(&self, run_id: &str, active: &ActiveRun) -> bool {
        let repo_path = self.project_path_for(run_id);
        !repo_path.is_empty()
            && Self::canonical_root(std::path::Path::new(&repo_path))
                == Self::canonical_root(&active.worktree.path)
    }

    /// The live run that owns a project's primary checkout, if one has been
    /// adopted. A terminal run has let go of it, so the checkout is adoptable
    /// again.
    pub(in crate::app) fn primary_run_of(&self, project_id: &str) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && self.projects.project_id_of(run_id) == Some(project_id)
                    && self.owns_primary_checkout(run_id, active)
            })
            .map(|(run_id, _)| run_id.clone())
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
