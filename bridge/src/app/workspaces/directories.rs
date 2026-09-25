//! `workspace.add_directory` / `workspace.remove_directory`: what a workspace
//! holds, after it was cut.
//!
//! A workspace is cut from the project's sources as they stood that day. These
//! two verbs are how it gains a directory afterwards and how it loses one,
//! without going back to the project — a project edit is forward-looking and
//! rewrites nothing that already exists.
//!
//! Split the way every filesystem verb here is: the decide half runs under the
//! app mutex and does nothing but resolve what is being added or removed and
//! write the record down; the git and the copy run off the mutex through the
//! deferred drain, because a clone waits on a network and walking a tree away
//! is unbounded. The directory is `pending` in the record until it lands, so a
//! reader between the two halves sees a directory being made rather than one
//! that is not there.

use super::{materialize_source, workspace_json};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{expand_tilde, require_str, AppState, DeferredGit, DeferredWork};
use crate::isolation::Isolation;
use crate::lifecycle::{CloneRepo, WorktreeMutation};
use crate::workspace::{
    MaterializedDirectory, Workspace, WorkspaceDirectory, WorkspaceRegistry, WorkspaceSource,
};
use crate::worktree::WorktreeManager;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What one added directory is made from.
///
/// A local source — a project's, or a folder the user browsed to — is the
/// per-source work creation already does. A remote has nothing on disk to
/// stand over yet, so it is cloned into the workspace itself and is its own
/// repository from then on.
enum DirectorySeed {
    Local(WorkspaceSource),
    Remote {
        url: String,
        requested_base: Option<String>,
        /// What the directory is recorded as before the clone answers: its
        /// name, its mount, and no path at all.
        source: WorkspaceSource,
    },
}

/// What the drain makes, resolved before the mutex was released.
struct AddWorkspaceDirectory {
    registry_root: PathBuf,
    rift_root: PathBuf,
    workspace_id: String,
    /// The name the checkout's branch is cut under — the workspace's own
    /// directory name, exactly as creation names it.
    workspace_name: String,
    directory_id: String,
    seed: DirectorySeed,
    isolation: Isolation,
}

impl DeferredGitWork for AddWorkspaceDirectory {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let mut registry = WorkspaceRegistry::load(&self.registry_root)?;
        let workspace = registry.materialize_directory(
            &self.workspace_id,
            &self.directory_id,
            |directory| self.make(directory),
        )?;
        Ok(workspace_json(&workspace))
    }

    /// A failed add removes its own record, so the in-memory index has to
    /// agree with what is left on disk either way.
    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn invalidate(&self, app: &mut AppState) {
        reload(app);
    }

    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        reload(app);
        app.workspace_get(&json!({ "workspace_id": self.workspace_id }))
    }
}

impl AddWorkspaceDirectory {
    fn make(&self, directory: &WorkspaceDirectory) -> Result<MaterializedDirectory, String> {
        match &self.seed {
            DirectorySeed::Local(source) => {
                let (branch, isolation) = materialize_source(
                    source,
                    &directory.path,
                    &self.workspace_name,
                    self.isolation,
                    &self.rift_root,
                )?;
                Ok(MaterializedDirectory {
                    branch,
                    isolation,
                    source_path: None,
                    base_branch: None,
                })
            }
            DirectorySeed::Remote {
                url,
                requested_base,
                ..
            } => {
                let root = directory
                    .path
                    .parent()
                    .unwrap_or(&directory.path)
                    .to_path_buf();
                let mount = directory
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let opened = CloneRepo {
                    url: url.clone(),
                    name: mount,
                    dest: directory.path.clone(),
                    projects_dir: root,
                    requested_base: requested_base.clone(),
                }
                .perform()?
                .output;
                // The clone is its own repository: it was cut from nowhere on
                // this device, so it is what it was made from.
                Ok(MaterializedDirectory {
                    branch: Some(opened.base.clone()),
                    isolation: Isolation::Worktree,
                    source_path: Some(opened.path),
                    base_branch: Some(opened.base),
                })
            }
        }
    }
}

/// What the drain removes, resolved before the mutex was released.
struct RemoveWorkspaceDirectory {
    registry_root: PathBuf,
    rift_root: PathBuf,
    workspace_id: String,
    directory: WorkspaceDirectory,
    retirements: Vec<crate::reaper::Retirement>,
}

impl DeferredGitWork for RemoveWorkspaceDirectory {
    fn run(&self, _: &Value) -> Result<Value, String> {
        for retirement in &self.retirements {
            if !retirement.wait(crate::orchestrator::CHECKOUT_REAP_WAIT) {
                return Err("Removing the directory stopped because a process did not exit; its files were preserved. Retry the removal".into());
            }
        }
        unmaterialize_directory(&self.directory, &self.rift_root)?;
        let mut registry = WorkspaceRegistry::load(&self.registry_root)?;
        let workspace = registry.forget_directory(&self.workspace_id, &self.directory.id)?;
        Ok(workspace_json(&workspace))
    }

    /// A partial removal is still a move: the index has to agree with what is
    /// left on disk before the next read is answered.
    fn invalidates_on_error(&self) -> bool {
        true
    }

    fn invalidate(&self, app: &mut AppState) {
        reload(app);
    }

    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        reload(app);
        app.workspace_get(&json!({ "workspace_id": self.workspace_id }))
    }
}

/// Hand one directory back to what it was made from and take the folder.
///
/// The same two steps `workspace.delete` takes over a whole root, narrowed to
/// one: unregister first, because a worktree walked away without telling its
/// repository leaves an administrative record pointing at nothing, and that
/// makes the same name unusable next time. A copy and a clone are nobody's
/// worktree and only have the folder to lose. The branch is left standing —
/// what a checkout put on it is the user's, and Delete leaves it too.
fn unmaterialize_directory(directory: &WorkspaceDirectory, rift_root: &Path) -> Result<(), String> {
    let path = &directory.path;
    if directory.is_git && Isolation::of(path).is_some() {
        WorktreeManager::new(&directory.source_path, path.parent().unwrap_or(path))
            .with_rift_registry_root(rift_root)
            .remove_checkout(path)
            .map_err(|error| {
                format!(
                    "Removing the directory failed at {}: {error}. Retry the removal",
                    path.display()
                )
            })?;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Removing the directory failed at {}: {error}. Retry the removal",
            path.display()
        )),
    }
}

/// The manifest was rewritten off the mutex, so the index is read again before
/// anything answers from it.
fn reload(app: &mut AppState) {
    if let Err(error) = app.workspaces.reload() {
        eprintln!("reload workspaces after a directory change: {error}");
    }
}

impl AppState {
    /// `workspace.add_directory` — one more directory in a workspace that is
    /// already standing.
    ///
    /// The directory is named three ways: a project source this workspace was
    /// not cut with, a path on the device, or a remote to clone. Whichever it
    /// is, what lands is the per-source work creation does, so an added Git
    /// directory is a checkout on a branch of the workspace's own cut from
    /// that source's base branch.
    ///
    /// Answers `workspace.get`'s shape from the drain: one read repaints every
    /// surface that names the workspace's directories.
    pub(crate) fn workspace_add_directory(&mut self, params: &Value) -> Result<Value, String> {
        let workspace = self.workspace_to_change(params)?;
        let seed = self.directory_seed(&workspace, params)?;
        let directory = self
            .workspaces
            .begin_directory(&workspace.id, seed.source())?;
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(AddWorkspaceDirectory {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&workspace.project_id),
                workspace_id: workspace.id.clone(),
                workspace_name: workspace
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                directory_id: directory.id,
                isolation: workspace.isolation,
                seed,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace.id, "pending": true }))
    }

    /// `workspace.remove_directory` — one directory leaves a workspace.
    ///
    /// Everything standing IN that directory stops first; a terminal whose cwd
    /// is elsewhere in the workspace is left working. The refusals are the ones
    /// removing a whole workspace applies, narrowed to one directory:
    /// [`AppState::refuse_removing_a_directory_that_is_not_builds`], and an
    /// agent working at the workspace root — its session is keyed at the root,
    /// so it is working in every directory of the workspace at once and the
    /// files being removed are the ones it is writing.
    pub(crate) fn workspace_remove_directory(&mut self, params: &Value) -> Result<Value, String> {
        let workspace = self.workspace_to_change(params)?;
        let directory_id = require_str(params, "directory_id")?;
        let directory = workspace
            .directories
            .iter()
            .find(|directory| directory.id == directory_id || directory.source_id == directory_id)
            .cloned()
            .ok_or_else(|| format!("unknown directory_id: {directory_id}"))?;
        let path = Self::canonical_root(&directory.path);
        self.refuse_removing_a_directory_that_is_not_builds(&workspace, &path)?;
        if self.agent_working_at_root(&Self::canonical_root(&workspace.root)) {
            return Err("Stop running agents before removing the directory".to_string());
        }
        let retirements = self.retire_everything_at(&path);
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(RemoveWorkspaceDirectory {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&workspace.project_id),
                workspace_id: workspace.id.clone(),
                directory,
                retirements,
            }),
            params: params.clone(),
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(json!({ "workspace_id": workspace.id, "pending": true }))
    }

    /// Everything that makes one directory not Build's to remove: the guards
    /// [`AppState::refuse_removing_what_is_not_builds`] reads over a whole
    /// root, narrowed to the one folder that is going.
    ///
    /// An adopted workspace is somebody else's working copy — Build found it,
    /// it did not make it — and the workspace Build adopts for a legacy
    /// project stands ON that project's repository, so its one directory is
    /// the source itself. A directory that resolves outside the root, or that
    /// holds a registered source repository, would take work with it that was
    /// never the workspace's.
    fn refuse_removing_a_directory_that_is_not_builds(
        &self,
        workspace: &Workspace,
        path: &Path,
    ) -> Result<(), String> {
        if !workspace.managed {
            return Err(
                "workspace.remove_directory: adopted checkouts are not Build's to remove"
                    .to_string(),
            );
        }
        let root = Self::canonical_root(&workspace.root);
        if !path.starts_with(&root) {
            return Err(
                "Cannot remove a workspace directory that resolves outside its workspace root"
                    .to_string(),
            );
        }
        if path == root {
            return Err(
                "Cannot remove a workspace directory that is the workspace root".to_string(),
            );
        }
        if self
            .projects
            .iter()
            .flat_map(|project| {
                std::iter::once(&project.repo_path)
                    .chain(project.sources.iter().map(|source| &source.path))
            })
            .any(|source| Self::canonical_root(source).starts_with(path))
        {
            return Err(
                "Cannot remove a workspace directory containing a source repository".to_string(),
            );
        }
        Ok(())
    }

    /// The workspace a directory verb was pointed at, with the filesystem free
    /// to change it.
    fn workspace_to_change(&mut self, params: &Value) -> Result<Workspace, String> {
        let workspace_id = require_str(params, "workspace_id")?;
        if self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || self.workspace_reserved(&workspace_id)
        {
            return Err(crate::reclaim::BUSY.to_string());
        }
        if self.workspaces.get(&workspace_id).is_none() {
            self.adopt_legacy_workspaces();
        }
        self.workspaces
            .get(&workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))
    }

    /// What the caller asked for one added directory to be made from, and the
    /// folder it mounts under — unique among the ones the workspace already
    /// has, so two directories named the same land beside each other.
    fn directory_seed(
        &self,
        workspace: &Workspace,
        params: &Value,
    ) -> Result<DirectorySeed, String> {
        let requested_name = params
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let base_branch = crate::app::projects::requested_base_branch(params);
        if let Some(source_id) = params.get("source_id").and_then(Value::as_str) {
            let source = self
                .sources_for(&workspace.project_id)?
                .into_iter()
                .find(|source| source.id == source_id)
                .ok_or_else(|| {
                    format!(
                        "unknown source_id {source_id} in project {}",
                        workspace.project_id
                    )
                })?;
            let name = requested_name.unwrap_or(&source.name).to_string();
            return Ok(DirectorySeed::Local(WorkspaceSource {
                mount: self.free_mount(workspace, &name),
                id: source.id,
                name,
                path: source.path,
                is_git: source.is_git,
                base_branch: base_branch.unwrap_or(source.base_branch),
            }));
        }
        if let Some(path) = params
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = crate::app::projects::canonical_source_path(&expand_tilde(path))?;
            self.refuse_another_projects_source(&workspace.project_id, &path)?;
            let inferred = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "directory".to_string());
            let name = requested_name.unwrap_or(&inferred).to_string();
            let mount = self.free_mount(workspace, &name);
            let is_git = path.join(".git").exists();
            return Ok(DirectorySeed::Local(WorkspaceSource {
                id: format!("dir-{mount}"),
                mount,
                name,
                is_git,
                base_branch: base_branch
                    .or_else(|| crate::worktree::git_default_branch(&path))
                    .unwrap_or_else(|| "main".to_string()),
                path,
            }));
        }
        let url = params
            .get("remote")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                "workspace.add_directory: name a source_id, a path or a remote".to_string()
            })?;
        let inferred = crate::app::projects::repo_name_from_url(url);
        let name = requested_name.unwrap_or(&inferred).to_string();
        let mount = self.free_mount(workspace, &name);
        Ok(DirectorySeed::Remote {
            url: url.to_string(),
            requested_base: base_branch,
            source: WorkspaceSource {
                id: format!("dir-{mount}"),
                mount,
                name,
                path: PathBuf::new(),
                is_git: true,
                base_branch: String::new(),
            },
        })
    }

    /// A directory named by path must not be another project's source. A
    /// Git source becomes a checkout, which cuts a branch and writes a worktree
    /// registration into the repository it was cut from: a workspace has no
    /// claim to do that in a project it was not cut in. `project.add_source`
    /// refuses the same shape in the same words, so both doors into the
    /// capability agree.
    fn refuse_another_projects_source(&self, project_id: &str, path: &Path) -> Result<(), String> {
        let overlaps = self
            .projects
            .iter()
            .filter(|project| project.id != project_id)
            .flat_map(|project| {
                std::iter::once(&project.repo_path)
                    .chain(project.sources.iter().map(|source| &source.path))
            })
            .any(|source| {
                let source = Self::canonical_root(source);
                path.starts_with(&source) || source.starts_with(path)
            });
        if overlaps {
            return Err(format!(
                "source overlaps a registered project source: {}",
                path.display()
            ));
        }
        Ok(())
    }

    /// A folder name inside this workspace root that nothing is standing on.
    fn free_mount(&self, workspace: &Workspace, name: &str) -> String {
        let base = crate::app::projects::safe_mount_name(name);
        let taken = |candidate: &str| {
            workspace
                .directories
                .iter()
                .any(|directory| directory.path == workspace.root.join(candidate))
        };
        let mut mount = base.clone();
        let mut suffix = 2;
        while taken(&mount) {
            mount = format!("{base}-{suffix}");
            suffix += 1;
        }
        mount
    }
}

impl DirectorySeed {
    /// The source record the directory is registered from, whichever door it
    /// came in by. A remote's path is empty until the clone answers.
    fn source(&self) -> &WorkspaceSource {
        match self {
            DirectorySeed::Local(source) | DirectorySeed::Remote { source, .. } => source,
        }
    }
}
