use super::source_directory_claims::SourceDirectoryLease;
use super::{
    canonical_source_path, repo_name_from_url, safe_mount_name, usable_project_name, ProjectSource,
};
use crate::app::config::accept_isolation;
use crate::app::{expand_tilde, require_str, AppState};
use crate::lifecycle::{
    CloneRepo, CreateRepo, InitializeRepo, OpenRepo, PendingRow, PendingState, Performed,
    WorktreeChange, WorktreeMutation,
};
use crate::remote_url::usable_remote_url;
use serde_json::Value;

struct SourceRequest {
    /// Preparation-local identity. Appended references receive final IDs under
    /// the app lock when their membership transaction publishes.
    id: String,
    path: Option<std::path::PathBuf>,
    remote: Option<String>,
    name: String,
    mount: String,
    base_branch: Option<String>,
    destination: Option<std::path::PathBuf>,
}

/// The names a set of sources has already used: where they stand on disk, the
/// remotes they clone, and the folder each one mounts under. A source is held
/// to this as well as to every registered project, so two sources opened
/// together cannot collide any more than two opened apart.
#[derive(Default)]
struct TakenSourceNames {
    paths: Vec<std::path::PathBuf>,
    remotes: std::collections::HashSet<String>,
    mounts: std::collections::HashSet<String>,
}

impl TakenSourceNames {
    /// Seed the claim with what a project already holds, for a source being
    /// appended to it rather than opened with it.
    fn over(sources: &[ProjectSource]) -> Self {
        TakenSourceNames {
            paths: sources
                .iter()
                .map(|source| crate::worktree::canonical_planned_path(&source.path))
                .collect(),
            remotes: sources.iter().filter_map(ProjectSource::origin).collect(),
            mounts: sources.iter().map(|source| source.mount.clone()).collect(),
        }
    }

    fn claim_path(&mut self, candidate: &std::path::Path) -> Result<(), String> {
        if self.paths.iter().any(|existing| candidate == existing) {
            return Err(format!(
                "source is already included in this project: {}",
                candidate.display()
            ));
        }
        if self.paths.iter().any(|existing: &std::path::PathBuf| {
            candidate.starts_with(existing) || existing.starts_with(candidate)
        }) {
            return Err(format!(
                "source overlaps another source: {}",
                candidate.display()
            ));
        }
        self.paths.push(candidate.to_path_buf());
        Ok(())
    }

    fn claim_remote(&mut self, remote: &str) -> Result<(), String> {
        if !self.remotes.insert(remote.to_string()) {
            return Err(format!("duplicate source remote: {remote}"));
        }
        Ok(())
    }

    /// Where each remote source will be cloned, held to the directories the
    /// local sources stand over: a clone must not land inside one of them.
    fn refuse_managed_destinations(
        &self,
        requests: &[SourceRequest],
        managed_root: &std::path::Path,
        registered: &[ProjectSource],
    ) -> Result<(), String> {
        for request in requests.iter().filter(|request| request.remote.is_some()) {
            let destination =
                crate::worktree::canonical_planned_path(&managed_root.join(&request.mount));
            if self
                .paths
                .iter()
                .chain(registered.iter().map(|source| &source.path))
                .any(|source| destination.starts_with(source) || source.starts_with(&destination))
            {
                return Err(format!(
                    "managed source destination overlaps a local source: {}",
                    destination.display()
                ));
            }
        }
        Ok(())
    }

    /// The folder this source mounts under, made unique: two sources named the
    /// same land beside each other rather than on top of each other.
    fn claim_mount(&mut self, name: &str) -> String {
        let base = safe_mount_name(name);
        let mut mount = base.clone();
        let mut suffix = 2;
        while !self.mounts.insert(mount.clone()) {
            mount = format!("{base}-{suffix}");
            suffix += 1;
        }
        mount
    }
}

struct PreparedSources {
    opened: crate::lifecycle::OpenedRepository,
    sources: Vec<ProjectSource>,
    created_checkouts: Vec<std::path::PathBuf>,
    cloned_paths: Vec<std::path::PathBuf>,
    _destination_claims: Vec<SourceDirectoryLease>,
}

struct OpenProjectSources {
    requests: Vec<SourceRequest>,
    managed_root: std::path::PathBuf,
    destination_claims: Vec<SourceDirectoryLease>,
}

fn open_requested_source(
    request: &SourceRequest,
    managed_root: &std::path::Path,
) -> Result<Performed<crate::lifecycle::OpenedRepository>, String> {
    if let Some(path) = &request.path {
        return OpenRepo {
            path: canonical_source_path(path)?,
            requested_base: request.base_branch.clone(),
        }
        .perform();
    }
    let dest = request
        .destination
        .clone()
        .expect("clone destination was reserved");
    CloneRepo {
        url: request.remote.clone().expect("validated source remote"),
        name: request.mount.clone(),
        projects_dir: dest.parent().unwrap_or(managed_root).to_path_buf(),
        dest,
        requested_base: request.base_branch.clone(),
    }
    .perform()
}

impl WorktreeMutation for OpenProjectSources {
    type Output = PreparedSources;

    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let mut sources = Vec::with_capacity(self.requests.len());
        let mut primary = None;
        let mut created_checkouts = Vec::new();
        let mut cloned_paths = Vec::new();
        for request in self.requests {
            let result = open_requested_source(&request, &self.managed_root);
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
            cloned_paths.extend(request.remote.as_ref().map(|_| opened.path.clone()));
            if let Some(path) = &opened.created_checkout {
                created_checkouts.push(path.clone());
            }
            sources.push(ProjectSource::added(
                request.id,
                request.name,
                request.mount,
                opened.path.clone(),
                opened.is_git,
                opened.base.clone(),
            ));
            if primary.is_none() {
                primary = Some(opened);
            }
        }
        for claim in &self.destination_claims {
            claim.mark_prepared();
        }
        Ok(Performed {
            change: WorktreeChange::nothing(),
            output: PreparedSources {
                opened: primary.expect("non-empty sources"),
                sources,
                created_checkouts,
                cloned_paths,
                _destination_claims: self.destination_claims,
            },
        })
    }
}

struct ReservedCreateRepository {
    mutation: CreateRepo,
    destination_claims: Vec<SourceDirectoryLease>,
}

struct PreparedRepository {
    opened: crate::lifecycle::OpenedRepository,
    _destination_claims: Vec<SourceDirectoryLease>,
}

impl WorktreeMutation for ReservedCreateRepository {
    type Output = PreparedRepository;

    fn perform(self) -> Result<Performed<Self::Output>, String> {
        let performed = self.mutation.perform()?;
        for claim in &self.destination_claims {
            claim.mark_prepared();
        }
        Ok(Performed {
            change: performed.change,
            output: PreparedRepository {
                opened: performed.output,
                _destination_claims: self.destination_claims,
            },
        })
    }
}

impl AppState {
    /// A retained home may revive without an RPC source admission. Exclude
    /// recorded locations before any clone or initializer can write there.
    fn reserve_source_destinations(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Result<Vec<SourceDirectoryLease>, String> {
        for path in paths {
            self.validate_secret_source(path)?;
            if self.source_directory_has_owner(path) {
                return Err(format!(
                    "managed source destination overlaps a project or workspace location: {}",
                    path.display()
                ));
            }
        }
        self.source_directory_claims.reserve_destinations(paths)
    }

    fn reserve_source_opening(
        &self,
        mut requests: Vec<SourceRequest>,
        managed_root: std::path::PathBuf,
    ) -> Result<OpenProjectSources, String> {
        let mut destinations = Vec::new();
        for request in requests
            .iter_mut()
            .filter(|request| request.remote.is_some())
        {
            let path = crate::worktree::canonical_planned_path(&managed_root.join(&request.mount));
            destinations.push(path.clone());
            request.destination = Some(path);
        }
        Ok(OpenProjectSources {
            requests,
            managed_root,
            destination_claims: self.reserve_source_destinations(&destinations)?,
        })
    }

    /// Stored references also reserve membership when a folder was absent
    /// during projection. Include the live home fallback without adopting it.
    pub(super) fn project_reference_sources(
        &self,
        project_id: &str,
    ) -> Result<Vec<ProjectSource>, String> {
        let project = self.project_for(project_id)?;
        let environment = self
            .projects
            .environment(&project.environment_id)
            .ok_or_else(|| format!("unknown environment: {}", project.environment_id))?;
        Ok(environment
            .sources
            .iter()
            .chain(&project.added_sources)
            .chain(&project.sources)
            .cloned()
            .collect())
    }

    pub(super) fn refuse_source_duplicate(
        &self,
        project_id: &str,
        candidate: &std::path::Path,
        source_id: &str,
    ) -> Result<(), String> {
        let project = self.project_for(project_id)?;
        let included_environment = self
            .projects
            .environments_referencing(candidate)
            .iter()
            .any(|environment| environment.id == project.environment_id);
        let duplicate = included_environment
            && self
                .projects
                .source_repoint_duplicates(project_id, source_id, candidate);
        if duplicate {
            return Err(format!(
                "source is already included in this project: {}",
                candidate.display()
            ));
        }
        Ok(())
    }

    pub(super) fn refuse_source_containment(
        &self,
        candidate: &std::path::Path,
        replacing: Option<(&str, &str)>,
    ) -> Result<(), String> {
        let registered = match replacing {
            Some((project_id, source_id)) => self
                .projects
                .registered_sources_except(project_id, source_id),
            None => self.projects.registered_sources(),
        };
        if registered.iter().any(|source| {
            source.path != candidate
                && (candidate.starts_with(&source.path) || source.path.starts_with(candidate))
        }) {
            return Err(format!(
                "source overlaps a registered project source: {}",
                candidate.display()
            ));
        }
        Ok(())
    }

    pub(super) fn validate_prepared_sources(
        &self,
        project_id: Option<&str>,
        sources: &mut [ProjectSource],
        cloned_paths: &[std::path::PathBuf],
    ) -> Result<(), String> {
        let mut taken = match project_id {
            Some(id) => TakenSourceNames::over(&self.project_reference_sources(id)?),
            None => TakenSourceNames::default(),
        };
        for source in sources.iter_mut() {
            source.path = canonical_source_path(&source.path)?;
            self.validate_secret_source(&source.path)?;
            self.refuse_source_directory_claim(&source.path)?;
            taken.claim_path(&source.path)?;
            self.refuse_source_containment(&source.path, None)?;
        }
        self.refuse_duplicate_clone_remotes(sources, cloned_paths)
    }

    fn refuse_duplicate_clone_remotes(
        &self,
        sources: &[ProjectSource],
        cloned_paths: &[std::path::PathBuf],
    ) -> Result<(), String> {
        let registered = self.projects.registered_sources();
        for source in sources.iter().filter(|source| {
            cloned_paths
                .iter()
                .any(|path| crate::worktree::canonical_planned_path(path) == source.path)
        }) {
            if let Some(remote) = source.origin() {
                if registered
                    .iter()
                    .chain(sources)
                    .filter(|other| other.path != source.path)
                    .filter_map(ProjectSource::origin)
                    .any(|origin| crate::worktree::remotes_match(&origin, &remote))
                {
                    return Err(format!("source remote is already registered: {remote}"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn project_init_git(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
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
    pub(super) fn defer_project<T, S>(
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
        self.validate_secret_source(&dest)?;
        let row = PendingRow::on_directory(
            crate::worktree::external_worktree_id(&crate::worktree::canonical_planned_path(&dest)),
            title,
            state,
        );
        self.defer_lifecycle(row, mutation, settlement)
    }

    /// Create a brand-new git repo (with an initial commit so its base branch
    /// resolves and tasks can dispatch) inside `parent` — a browsed-to directory,
    /// or the projects folder by default — and register it. An optional `remote`
    /// is wired as `origin` at creation.
    pub(crate) fn project_create(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        // A folder that is back answers as the project it was, not a new one.
        self.revive_parked_projects();
        let name = usable_project_name(require_str(params, "name")?)?;
        if let Some(environment) = params["environment_id"].as_str() {
            return self.project_create_in_environment(environment, &name, params);
        }
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
        let dest = crate::worktree::canonical_planned_path(&parent.join(&name));
        let remote = params
            .get("remote")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|remote| !remote.is_empty())
            .map(usable_remote_url)
            .transpose()?;
        let destination_claims = self.reserve_source_destinations(std::slice::from_ref(&dest))?;
        self.defer_project(
            dest.clone(),
            name.clone(),
            PendingState::Creating,
            ReservedCreateRepository {
                mutation: CreateRepo {
                    name,
                    dest,
                    base_branch,
                    remote,
                },
                destination_claims,
            },
            |state: &mut AppState, result: Result<PreparedRepository, String>| {
                let prepared = result?;
                state.register_opened_repository(prepared.opened)
            },
        )
    }

    /// One source of a project, read off the wire and held to every source
    /// already registered and every source being opened beside it. The checks
    /// are the same whether the source is one a project is opened over or one
    /// appended to a project that already exists.
    fn source_request(
        &self,
        entry: &Value,
        id: String,
        taken: &mut TakenSourceNames,
    ) -> Result<SourceRequest, String> {
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
        let remote = remote.map(usable_remote_url).transpose()?;
        let remote = remote.as_deref();
        let path = path_text.map(expand_tilde);
        let canonical = path.as_deref().map(canonical_source_path).transpose()?;
        if let Some(candidate) = &canonical {
            self.validate_secret_source(candidate)?;
            self.refuse_source_directory_claim(candidate)?;
            taken.claim_path(candidate)?;
            self.refuse_source_containment(candidate, None)?;
        }
        if let Some(remote) = remote {
            if self
                .projects
                .registered_sources()
                .iter()
                .filter_map(ProjectSource::origin)
                .any(|origin| crate::worktree::remotes_match(&origin, remote))
            {
                return Err(format!("source remote is already registered: {remote}"));
            }
            taken.claim_remote(remote)?;
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
        let mount = taken.claim_mount(&name);
        Ok(SourceRequest {
            id,
            path: canonical,
            remote: remote.map(str::to_string),
            name,
            mount,
            destination: None,
            base_branch: entry
                .get("base_branch")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        })
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
        let mut taken = TakenSourceNames::default();
        let ids = self.projects.allocate_source_ids(None, entries.len())?;
        for (entry, id) in entries.iter().zip(ids) {
            requests.push(self.source_request(entry, id, &mut taken)?);
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
        let canonical_projects_dir = crate::worktree::canonical_planned_path(&self.projects_dir);
        let planned_managed_root = canonical_projects_dir.join(format!("{project_name}-sources"));
        taken.refuse_managed_destinations(
            &requests,
            &planned_managed_root,
            &self.projects.registered_sources(),
        )?;
        let title = project_name;
        let mutation = self.reserve_source_opening(requests, managed_root.clone())?;
        self.defer_project(
            managed_root.clone(),
            title,
            PendingState::Creating,
            mutation,
            |state: &mut AppState, result: Result<PreparedSources, String>| {
                let prepared = result?;
                state.register_opened_project_sources(
                    prepared.opened,
                    prepared.sources,
                    prepared.created_checkouts,
                    prepared.cloned_paths,
                )
            },
        )
    }

    /// `project.add_source` — one more directory on a project that already
    /// exists.
    ///
    /// The same open the project was registered with, the same validation, and
    /// the same off-lock drain: a local path is opened where it stands, a
    /// remote is cloned into the project's own sources folder. Admission uses
    /// the environment's complete ID reservations; settlement assigns final IDs
    /// again under the app lock, so concurrent preparation cannot reuse a
    /// published reference. The durable counter retains removed identities.
    ///
    /// Forward-looking, like every project edit: a workspace already cut keeps
    /// the directories it was cut with, and `workspace.add_directory` is how
    /// one of them gets this folder.
    pub(crate) fn project_add_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let project_id = require_str(params, "project_id")?;
        let project = self.project_for(&project_id)?;
        let project_name = usable_project_name(safe_mount_name(&project.name))?;
        let sources = self.project_reference_sources(&project_id)?;
        let id = self
            .projects
            .allocate_source_ids(Some(&project_id), 1)?
            .pop()
            .expect("one source ID requested");
        let mut taken = TakenSourceNames::over(&sources);
        let request = self.source_request(params, id, &mut taken)?;
        let managed_root = self.projects_dir.join(format!("{project_name}-sources"));
        let canonical_projects_dir = crate::worktree::canonical_planned_path(&self.projects_dir);
        let planned_managed_root = canonical_projects_dir.join(format!("{project_name}-sources"));
        taken.refuse_managed_destinations(
            std::slice::from_ref(&request),
            &planned_managed_root,
            &self.projects.registered_sources(),
        )?;
        // The directory the source will be read or written at is the row's
        // identity, as it is for every other project verb: what a second
        // `add_source` collides with is the folder, not the project's record.
        let dest = request
            .path
            .clone()
            .unwrap_or_else(|| managed_root.join(&request.mount));
        let mutation = self.reserve_source_opening(vec![request], managed_root)?;
        self.defer_project(
            dest,
            project.name.clone(),
            PendingState::Updating,
            mutation,
            move |state: &mut AppState, result: Result<PreparedSources, String>| {
                let prepared = result?;
                state.append_project_sources_with_clones(
                    &project_id,
                    prepared.sources,
                    prepared.created_checkouts,
                    prepared.cloned_paths,
                )
            },
        )
    }

    /// `project.remove_source` — the project stops cutting this folder into new
    /// workspaces. Nothing on disk moves: the folder is the user's, and every
    /// workspace already holding a directory from it keeps that directory.
    pub(crate) fn project_remove_source(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let project_id = require_str(params, "project_id")?;
        let source_id = require_str(params, "source_id")?;
        self.check_project_source_edit(&project_id, &source_id, false)?;
        let project = self.project_for(&project_id)?;
        if !project.sources.iter().any(|source| source.id == source_id) {
            return Err(format!(
                "unknown source_id {source_id} in project {project_id}"
            ));
        }
        if project.sources.len() == 1 {
            return Err("a project must have at least one source".to_string());
        }
        let source = project
            .sources
            .iter()
            .find(|source| source.id == source_id)
            .expect("source was checked");
        crate::app::fs::refuse_active_source(&source.path)?;
        self.refuse_source_shells_at(&source.path)?;

        self.refuse_moving_sources_under_workers(&project_id)?;
        let sources = project
            .sources
            .iter()
            .filter(|source| source.id != source_id)
            .cloned()
            .collect::<Vec<_>>();
        let value = self.commit_project_configuration(&project_id, |projects| {
            assert!(
                projects.set_sources(&project_id, sources),
                "project was resolved"
            );
            Ok(())
        })?;
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(value)
    }

    /// `project.set_remote`, for clients that predate per-source remotes: a
    /// project has no remote of its own, so this is its first source's,
    /// edited the way `project.update_source` edits any source's. An empty
    /// url takes `origin` off.
    pub(crate) fn project_set_remote(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
        let project_id = require_str(params, "project_id")?;
        let url = require_str(params, "url")?;
        let project = self
            .projects
            .get(&project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        if !project.is_git {
            return Err("project is not a git repository; initialize Git first".to_string());
        }
        let source_id = project
            .sources
            .first()
            .map(|source| source.id.clone())
            .ok_or_else(|| "a project must have at least one source".to_string())?;
        self.project_update_source(&serde_json::json!({
            "project_id": project_id,
            "source_id": source_id,
            "remote": url,
        }))
    }

    /// Set (or clear, with a null isolation) a project's override of the
    /// account's isolation. The choice is put to this project's volume before
    /// it is stored, so a client only ever repaints from a row the bridge would
    /// honour; naming no isolation at all is a missing param, not a clear.
    pub(crate) fn project_set_isolation(&mut self, params: &Value) -> Result<Value, String> {
        self.mark_device_lifecycle_dirty()?;
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
        let value = self.commit_project_configuration(&project_id, |projects| {
            assert!(
                projects.set_isolation(&project_id, isolation),
                "project was resolved"
            );
            Ok(())
        })?;
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(value)
    }
}

impl AppState {
    pub(in crate::app) fn plan_creation_sources(
        &self,
        entries: &[Value],
        inherited: &[ProjectSource],
        managed_root: &std::path::Path,
    ) -> Result<Value, String> {
        let mut taken = TakenSourceNames::over(inherited);
        let first = super::next_source_number(inherited);
        let mut requests = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            requests.push(self.source_request(
                entry,
                format!("source-{}", first + index as u64),
                &mut taken,
            )?);
        }
        taken.refuse_managed_destinations(
            &requests,
            managed_root,
            &self.projects.registered_sources(),
        )?;
        for request in requests.iter().filter(|r| r.remote.is_some()) {
            let destination = managed_root.join(&request.mount);
            self.validate_secret_source(&destination)?;
            if self.source_directory_has_owner(&destination) {
                return Err(
                    "managed source destination overlaps a project or workspace location".into(),
                );
            }
        }
        let planned = requests.into_iter().map(|request| {
            let path = request.path.unwrap_or_else(|| managed_root.join(&request.mount));
            serde_json::json!({"source":ProjectSource::added(request.id,request.name,request.mount,path,false,request.base_branch.clone().unwrap_or_else(||"main".into())),"remote":request.remote,"base_branch":request.base_branch})
        }).collect::<Vec<_>>();
        Ok(serde_json::json!(planned))
    }
}

impl AppState {
    fn project_create_in_environment(
        &mut self,
        environment: &str,
        name: &str,
        params: &Value,
    ) -> Result<Value, String> {
        self.refuse_pending_environment_creation(environment)?;
        let inherited = self
            .projects
            .environment(environment)
            .ok_or("unknown environment")?
            .sources
            .clone();
        let entries = params["sources"].as_array().cloned().unwrap_or_default();
        let key = format!("project:{}", uuid::Uuid::new_v4());
        if entries.is_empty() {
            let provisional = format!("proj-{}", self.projects.next_id());
            let id = self.create_environment_project(
                environment,
                name,
                &key,
                &provisional,
                Vec::new(),
                |_| Ok(()),
            )?;
            return Ok(self.project_json(self.projects.get(&id).expect("published project")));
        }
        let mut taken = TakenSourceNames::over(&inherited);
        let first = super::next_source_number(&inherited);
        let requests = entries
            .iter()
            .enumerate()
            .map(|(n, e)| {
                self.source_request(e, format!("source-{}", first + n as u64), &mut taken)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let root = self
            .projects_dir
            .join(format!("{}-sources", safe_mount_name(name)));
        taken.refuse_managed_destinations(
            &requests,
            &crate::worktree::canonical_planned_path(&root),
            &self.projects.registered_sources(),
        )?;
        let mutation = self.reserve_source_opening(requests, root.clone())?;
        let environment = environment.to_owned();
        let name = name.to_owned();
        self.defer_project(
            root,
            name.clone(),
            PendingState::Creating,
            mutation,
            move |app: &mut AppState, result: Result<PreparedSources, String>| {
                let prepared = result?;
                let result = (|| {
                    let mut added = prepared.sources;
                    app.validate_creation_additions(
                        &environment,
                        &mut added,
                        &prepared.cloned_paths,
                    )?;
                    let provisional = format!("proj-{}", app.projects.next_id());
                    let id = app.create_environment_project(
                        &environment,
                        &name,
                        &key,
                        &provisional,
                        added,
                        |_| Ok(()),
                    )?;
                    Ok(app.project_json(app.projects.get(&id).expect("published project")))
                })();
                if result.is_err() {
                    app.cleanup_unregistered_sources(prepared.created_checkouts);
                }
                result
            },
        )
    }
}
