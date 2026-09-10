use super::{repo_name_from_url, requested_base_branch, usable_project_name};
use crate::app::config::accept_isolation;
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{
    CloneRepo, CreateRepo, OpenRepo, PendingRow, PendingState, SetRemote, WorktreeMutation,
};
use crate::worktree::git_remote_origin;
use serde_json::Value;

impl AppState {
    /// Register a project from a host path. Validates it is a git repo with the
    /// requested base branch before adding, so a bad path fails loudly here rather
    /// than at first dispatch.
    pub(in crate::app) fn project_add(&mut self, params: &Value) -> Result<Value, String> {
        let path = require_str(params, "path")?;
        let path = expand_tilde(&path);
        self.defer_project(
            path.clone(),
            path.display().to_string(),
            PendingState::Creating,
            Box::new(OpenRepo {
                requested_base: requested_base_branch(params),
                path,
            }),
        )
    }

    /// Reserve the directory a project verb is about to read or write and hand
    /// its git to the drain. The directory is the row's identity: there is no
    /// project id until the git lands, and what two project verbs collide over
    /// is the folder, not a name.
    fn defer_project(
        &mut self,
        dest: std::path::PathBuf,
        title: String,
        state: PendingState,
        mutation: Box<dyn WorktreeMutation>,
    ) -> Result<Value, String> {
        let row = PendingRow::on_directory(
            crate::worktree::external_worktree_id(&crate::worktree::canonical_planned_path(&dest)),
            title,
            state,
        );
        self.defer_lifecycle(row, mutation)
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
            Box::new(CloneRepo {
                url,
                name,
                dest,
                projects_dir: self.projects_dir.clone(),
                requested_base: requested_base_branch(params),
            }),
        )
    }

    /// Create a brand-new git repo (with an initial commit so its base branch
    /// resolves and tasks can dispatch) inside `parent` — a browsed-to directory,
    /// or the projects folder by default — and register it. An optional `remote`
    /// is wired as `origin` at creation.
    pub(in crate::app) fn project_create(&mut self, params: &Value) -> Result<Value, String> {
        let name = usable_project_name(require_str(params, "name")?)?;
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
            Box::new(CreateRepo {
                name,
                dest,
                base_branch,
                remote: params
                    .get("remote")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|remote| !remote.is_empty())
                    .map(str::to_string),
            }),
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
            Box::new(SetRemote {
                project_id,
                repo_path,
                url: url.trim().to_string(),
            }),
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
