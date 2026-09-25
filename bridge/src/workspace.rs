//! Durable multi-directory workspace records.
//!
//! A workspace owns a container directory and snapshots the project sources
//! materialized beneath it.  The manifest lives with the workspace so a bridge
//! restart can recover a fully-created or interrupted workspace without
//! guessing from directory names.

use crate::isolation::Isolation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;

pub const MANIFEST_FILE: &str = ".build-workspace.json";
pub(crate) const PENDING_MARKER_FILE: &str = ".build-workspace.pending";

pub(crate) fn is_managed_workspace_mount(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.join(PENDING_MARKER_FILE).is_file() || parent.join(MANIFEST_FILE).is_file()
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Provisioning,
    Ready,
    Finished,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectoryStatus {
    Pending,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDirectory {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub path: PathBuf,
    pub is_git: bool,
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_isolation: Option<Isolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_head: Option<String>,
    pub status: DirectoryStatus,
    /// Immutable provisioning inputs retained for restart retry.
    pub source_path: PathBuf,
    pub base_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub root: PathBuf,
    pub status: WorkspaceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    pub directories: Vec<WorkspaceDirectory>,
    #[serde(default)]
    pub isolation: Isolation,
    /// False for adopted Git-root checkouts, which are never written into.
    #[serde(default)]
    pub managed: bool,
    /// True when an agent asked Build to create this workspace. Persisted so
    /// an agentless workspace stays muted across board pulls and restarts.
    #[serde(default)]
    pub created_by_agent: bool,
}

/// Source snapshot used while provisioning. Remote sources have already been
/// localized by project creation; workspace creation never mutates a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSource {
    pub id: String,
    pub name: String,
    pub mount: String,
    pub path: PathBuf,
    pub is_git: bool,
    pub base_branch: String,
}

/// What materializing one directory settled. `source_path` and `base_branch`
/// are written back only by a directory that learned them while it was being
/// made — a clone knows neither until the remote has answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedDirectory {
    pub branch: Option<String>,
    pub isolation: Isolation,
    pub source_path: Option<PathBuf>,
    pub base_branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepositoryFinish {
    pub directory_id: String,
    pub pushed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceFinish {
    pub complete: bool,
    pub repositories: Vec<RepositoryFinish>,
}

/// In-memory index over manifests stored beneath a configured workspace root.
#[derive(Debug)]
pub struct WorkspaceRegistry {
    root: PathBuf,
    storage_anchor: crate::reclaim::containment::StorageAnchor,
    workspaces: HashMap<String, Workspace>,
}

impl WorkspaceRegistry {
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        let mut registry = Self {
            storage_anchor: crate::reclaim::containment::StorageAnchor::new(&root),
            root,
            workspaces: HashMap::new(),
        };
        registry.reload()?;
        Ok(registry)
    }

    /// Startup-only load. A provisioning record at process boot has no live
    /// worker behind it, so it becomes a retryable interrupted workspace.
    pub fn recover(root: impl Into<PathBuf>) -> Result<Self, String> {
        let mut registry = Self::load(root)?;
        registry.recover_interrupted()?;
        Ok(registry)
    }

    pub fn empty(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            storage_anchor: crate::reclaim::containment::StorageAnchor::new(&root),
            root,
            workspaces: HashMap::new(),
        }
    }

    pub fn reload(&mut self) -> Result<(), String> {
        self.workspaces.retain(|_, workspace| !workspace.managed);
        if !self.root.exists() {
            return Ok(());
        }
        let registry_root = fs::canonicalize(&self.root).map_err(|error| {
            format!(
                "resolve workspace registry {}: {error}",
                self.root.display()
            )
        })?;
        for project_entry in read_dirs(&self.root)? {
            if project_entry
                .file_type()
                .map(|kind| kind.is_symlink())
                .unwrap_or(true)
                || !project_entry.path().is_dir()
            {
                continue;
            }
            for workspace_entry in read_dirs(&project_entry.path())? {
                if workspace_entry
                    .file_type()
                    .map(|kind| kind.is_symlink())
                    .unwrap_or(true)
                {
                    continue;
                }
                let manifest = workspace_entry.path().join(MANIFEST_FILE);
                if !manifest.is_file() {
                    continue;
                }
                let bytes = fs::read(&manifest).map_err(|error| {
                    format!("read workspace manifest {}: {error}", manifest.display())
                })?;
                let workspace: Workspace = serde_json::from_slice(&bytes).map_err(|error| {
                    format!("parse workspace manifest {}: {error}", manifest.display())
                })?;
                validate_loaded_workspace(
                    &workspace,
                    &workspace_entry.path(),
                    &project_entry.path(),
                    &registry_root,
                )?;
                self.storage_anchor
                    .capture_workspace(&workspace.root)
                    .map_err(|error| {
                        format!(
                            "remember workspace root {}: {error}",
                            workspace.root.display()
                        )
                    })?;
                self.workspaces.insert(workspace.id.clone(), workspace);
            }
        }
        let legacy = self.root.join(".legacy");
        for workspace in self
            .workspaces
            .values_mut()
            .filter(|workspace| !workspace.managed)
        {
            let proof = fs::read(legacy.join(&workspace.id))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Workspace>(&bytes).ok());
            workspace.status = if let Some(proof) = proof {
                workspace.archived_at = proof.archived_at.clone();
                for directory in &mut workspace.directories {
                    directory.finished_head = proof
                        .directories
                        .iter()
                        .find(|candidate| candidate.source_id == directory.source_id)
                        .and_then(|candidate| candidate.finished_head.clone());
                }
                WorkspaceStatus::Finished
            } else {
                workspace.archived_at = None;
                WorkspaceStatus::Ready
            };
        }
        Ok(())
    }

    fn recover_interrupted(&mut self) -> Result<(), String> {
        for workspace in self
            .workspaces
            .values_mut()
            .filter(|workspace| workspace.status == WorkspaceStatus::Provisioning)
        {
            workspace.status = WorkspaceStatus::Failed;
            for directory in workspace
                .directories
                .iter_mut()
                .filter(|directory| directory.status == DirectoryStatus::Pending)
            {
                directory.error = Some(if directory.path.exists() {
                    "creation was interrupted after this path appeared; inspect it and move it aside before retrying".to_string()
                } else {
                    "creation was interrupted; retry the workspace".to_string()
                });
            }
            persist(workspace)?;
        }
        Ok(())
    }

    pub fn claim_retry(&mut self, id: &str) -> Result<Workspace, String> {
        let workspace = self
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        match workspace.status {
            WorkspaceStatus::Failed => {}
            WorkspaceStatus::Provisioning => {
                return Err("workspace provisioning is already running".to_string())
            }
            _ => return Err("workspace has no failed provisioning to retry".to_string()),
        }
        let previous = workspace.clone();
        workspace.status = WorkspaceStatus::Provisioning;
        if let Err(error) = persist(workspace) {
            *workspace = previous;
            return Err(error);
        }
        Ok(workspace.clone())
    }

    pub fn fail_if_still_provisioning(&mut self, id: &str) {
        let Some(workspace) = self.workspaces.get_mut(id) else {
            return;
        };
        if workspace.status != WorkspaceStatus::Provisioning {
            return;
        }
        workspace.status = WorkspaceStatus::Failed;
        for directory in workspace
            .directories
            .iter_mut()
            .filter(|directory| directory.status == DirectoryStatus::Pending)
        {
            directory.error =
                Some("workspace provisioning failed; retry the workspace".to_string());
        }
        if let Err(error) = persist(workspace) {
            eprintln!("record failed workspace {id}: {error}");
        }
    }

    pub(crate) fn forget_project(&mut self, project_id: &str) {
        self.workspaces
            .retain(|_, workspace| workspace.project_id != project_id);
    }

    /// Give a workspace a new human-facing name.
    ///
    /// Only the label moves. The directory on disk and the branches inside it
    /// are what every terminal, worktree registration and running agent is
    /// already holding open, and the manifest is validated against its own
    /// containing directory on every reload — so renaming the folder would
    /// mean rebuilding the workspace, while renaming the record is a one-field
    /// write that survives the next load exactly as it was made.
    ///
    /// Adopted checkouts are named by the directory Build found them in and
    /// have no manifest to write, so there is nothing here to rename.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<Workspace, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("workspace.rename: name cannot be empty".to_string());
        }
        let workspace = self
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        if !workspace.managed {
            return Err(
                "workspace.rename: adopted workspaces are named by their own checkout".to_string(),
            );
        }
        workspace.name = name.to_string();
        persist(workspace)?;
        Ok(workspace.clone())
    }

    /// Drop one workspace from the index, its root having been removed. The
    /// manifest went with the directory, so the next reload agrees.
    pub(crate) fn forget(&mut self, id: &str) {
        self.workspaces.remove(id);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn storage_anchor(&self) -> crate::reclaim::containment::StorageAnchor {
        self.storage_anchor.clone()
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.get(id)
    }

    pub fn record_git_repository(
        &mut self,
        workspace_id: &str,
        source_id: &str,
        branch: &str,
    ) -> Result<Workspace, String> {
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        let directory = workspace
            .directories
            .iter_mut()
            .find(|directory| directory.source_id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in workspace {workspace_id}"))?;
        directory.is_git = true;
        directory.branch = Some(branch.to_string());
        directory.base_branch = branch.to_string();
        if workspace.managed {
            persist(workspace)?;
        }
        Ok(workspace.clone())
    }

    pub fn reopen(&mut self, id: &str) -> Result<(), String> {
        let workspace = self
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        if workspace.status == WorkspaceStatus::Finished {
            workspace.status = WorkspaceStatus::Ready;
            workspace.archived_at = None;
            if workspace.managed {
                persist(workspace)?;
            } else {
                let sidecar = self.root.join(".legacy").join(id);
                match fs::remove_file(sidecar) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(format!("reopen legacy workspace {id}: {error}")),
                }
            }
        }
        Ok(())
    }

    pub fn list(&self, project_id: Option<&str>) -> Vec<&Workspace> {
        let mut workspaces = self
            .workspaces
            .values()
            .filter(|workspace| project_id.is_none_or(|id| workspace.project_id == id))
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        workspaces
    }

    pub fn refresh_local_capabilities(&mut self) {
        for workspace in self.workspaces.values_mut() {
            let mut changed = false;
            for directory in &mut workspace.directories {
                let Ok(repository) = git2::Repository::open_ext(
                    &directory.path,
                    git2::RepositoryOpenFlags::NO_SEARCH,
                    std::iter::empty::<&Path>(),
                ) else {
                    continue;
                };
                let branch = repository
                    .head()
                    .ok()
                    .and_then(|head| head.shorthand().map(str::to_string));
                changed |= !directory.is_git || directory.branch != branch;
                directory.is_git = true;
                directory.branch = branch;
            }
            if changed && workspace.managed {
                if let Err(error) = persist(workspace) {
                    eprintln!("refresh workspace {}: {error}", workspace.id);
                }
            }
        }
    }

    /// Reopen a finished workspace when local state no longer matches the
    /// commit proof captured by Finish. No network access is performed here.
    pub fn refresh_finished_local(&mut self) {
        let finished = self
            .workspaces
            .values()
            .filter(|workspace| workspace.status == WorkspaceStatus::Finished)
            .map(|workspace| workspace.id.clone())
            .collect::<Vec<_>>();
        for id in finished {
            let stale = self.workspaces.get(&id).is_some_and(|workspace| {
                workspace.directories.iter().any(|directory| {
                    if !directory.path.is_dir() {
                        return true;
                    }
                    if !directory.is_git {
                        return false;
                    }
                    let head = git_output(&directory.path, &["rev-parse", "HEAD"]);
                    let dirty = git_output(&directory.path, &["status", "--porcelain"]);
                    head.ok().as_ref() != directory.finished_head.as_ref()
                        || dirty.is_err()
                        || dirty.is_ok_and(|status| !status.is_empty())
                })
            });
            if stale {
                if let Err(error) = self.reopen(&id) {
                    eprintln!("refresh finished workspace {id}: {error}");
                }
            }
        }
    }

    /// Reserve and persist a workspace before any expensive copy starts.
    pub fn begin(
        &mut self,
        project_id: &str,
        name: &str,
        sources: &[WorkspaceSource],
    ) -> Result<Workspace, String> {
        self.begin_with_isolation(project_id, name, sources, Isolation::default())
    }

    pub fn begin_with_isolation(
        &mut self,
        project_id: &str,
        name: &str,
        sources: &[WorkspaceSource],
        isolation: Isolation,
    ) -> Result<Workspace, String> {
        let workspace = self.prepare_with_isolation(project_id, name, sources, isolation)?;
        if let Err(error) = persist(&workspace) {
            let _ = fs::remove_dir_all(&workspace.root);
            return Err(error);
        }
        self.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        Ok(workspace)
    }

    /// Reserve an unpublished workspace container. New workspace creation
    /// uses this so list/get cannot observe a half-built record.
    pub fn prepare_with_isolation(
        &self,
        project_id: &str,
        name: &str,
        sources: &[WorkspaceSource],
        isolation: Isolation,
    ) -> Result<Workspace, String> {
        let mut mounts = std::collections::HashSet::new();
        for source in sources {
            validate_segment("source mount", &source.mount)?;
            if !mounts.insert(source.mount.as_str()) {
                return Err(format!("duplicate source mount: {:?}", source.mount));
            }
        }
        let project_root = self.root.join(project_id);
        fs::create_dir_all(&project_root).map_err(|error| {
            format!(
                "create workspace parent {}: {error}",
                project_root.display()
            )
        })?;
        self.storage_anchor.capture();
        // The name is user-facing text.  It must survive byte-for-byte in the
        // record, while only a bounded, portable derivative reaches a path.
        let root = create_unique_dir(&project_root, &crate::worktree::slugify(name))?;
        if let Err(error) = fs::write(root.join(PENDING_MARKER_FILE), b"") {
            let _ = fs::remove_dir_all(&root);
            return Err(format!(
                "mark pending workspace {}: {error}",
                root.display()
            ));
        }
        if let Err(error) = self.storage_anchor.register_workspace(&root) {
            let _ = fs::remove_dir_all(&root);
            return Err(format!(
                "remember new workspace root {}: {error}",
                root.display()
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let directories = sources
            .iter()
            .map(|source| WorkspaceDirectory {
                id: format!("{id}:{}", source.id),
                source_id: source.id.clone(),
                name: source.name.clone(),
                path: root.join(&source.mount),
                is_git: source.is_git,
                branch: None,
                effective_isolation: None,
                finished_head: None,
                status: DirectoryStatus::Pending,
                source_path: source.path.clone(),
                base_branch: source.base_branch.clone(),
                error: None,
            })
            .collect::<Vec<_>>();
        let workspace = Workspace {
            id: id.clone(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            root,
            status: WorkspaceStatus::Provisioning,
            archived_at: None,
            directories,
            isolation,
            managed: true,
            created_by_agent: false,
        };
        Ok(workspace)
    }

    /// Materialize and publish a workspace as one externally visible action.
    /// The error carries the updated private record so callers can unwind
    /// every checkout that succeeded before the failure.
    pub fn provision_unpublished<F>(
        &mut self,
        mut workspace: Workspace,
        sources: &[WorkspaceSource],
        isolation: Isolation,
        mut materialize: F,
    ) -> Result<Workspace, Box<(String, Workspace)>>
    where
        F: FnMut(&WorkspaceSource, &Path, Isolation) -> Result<(Option<String>, Isolation), String>,
    {
        for source in sources {
            let Some(directory) = workspace
                .directories
                .iter_mut()
                .find(|directory| directory.source_id == source.id)
            else {
                continue;
            };
            match materialize(source, &directory.path, isolation) {
                Ok((branch, effective_isolation)) => {
                    directory.branch = branch;
                    directory.effective_isolation = Some(effective_isolation);
                    directory.status = DirectoryStatus::Ready;
                }
                Err(error) => return Err(Box::new((error, workspace))),
            }
        }
        if workspace
            .directories
            .iter()
            .any(|directory| directory.status != DirectoryStatus::Ready)
        {
            return Err(Box::new((
                "workspace creation did not materialize every source".to_string(),
                workspace,
            )));
        }
        workspace.status = WorkspaceStatus::Ready;
        if let Err(error) = persist(&workspace) {
            return Err(Box::new((error, workspace)));
        }
        self.workspaces
            .insert(workspace.id.clone(), workspace.clone());
        Ok(workspace)
    }

    /// Provision pending directories and checkpoint after every source. A
    /// failed directory stays retryable across process restarts.
    pub fn provision<F>(
        &mut self,
        id: &str,
        sources: &[WorkspaceSource],
        isolation: Isolation,
        mut materialize: F,
    ) -> Result<Workspace, String>
    where
        F: FnMut(&WorkspaceSource, &Path, Isolation) -> Result<(Option<String>, Isolation), String>,
    {
        let workspace = self
            .workspaces
            .get_mut(id)
            .ok_or_else(|| format!("unknown workspace_id: {id}"))?;
        for source in sources {
            let Some(directory) = workspace
                .directories
                .iter_mut()
                .find(|directory| directory.source_id == source.id)
            else {
                continue;
            };
            if directory.status == DirectoryStatus::Ready {
                continue;
            }
            match materialize(source, &directory.path, isolation) {
                Ok((branch, effective_isolation)) => {
                    directory.branch = branch;
                    directory.effective_isolation = Some(effective_isolation);
                    directory.status = DirectoryStatus::Ready;
                    directory.error = None;
                }
                Err(error) => {
                    directory.status = DirectoryStatus::Failed;
                    directory.error = Some(error.clone());
                    workspace.status = WorkspaceStatus::Failed;
                    persist(workspace)?;
                    return Err(error);
                }
            }
            persist(workspace)?;
        }
        workspace.status = if workspace
            .directories
            .iter()
            .all(|directory| directory.status == DirectoryStatus::Ready)
        {
            WorkspaceStatus::Ready
        } else {
            WorkspaceStatus::Failed
        };
        persist(workspace)?;
        Ok(workspace.clone())
    }

    /// Record one more directory on a workspace, pending what makes it.
    ///
    /// The record is written before anything is on disk, exactly as creation
    /// writes the whole snapshot first: the manifest is what a restart reads,
    /// so a directory that is half-made must already be named in it.
    ///
    /// An adopted checkout has no manifest to write and was never Build's to
    /// lay out, so it has no directories to gain.
    pub fn begin_directory(
        &mut self,
        workspace_id: &str,
        source: &WorkspaceSource,
    ) -> Result<WorkspaceDirectory, String> {
        validate_segment("source mount", &source.mount)?;
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        if !workspace.managed {
            return Err(
                "workspace.add_directory: adopted checkouts are not Build's to change".to_string(),
            );
        }
        if workspace
            .directories
            .iter()
            .any(|directory| directory.source_id == source.id)
        {
            return Err(format!(
                "source {} is already a directory in workspace {workspace_id}",
                source.id
            ));
        }
        let path = workspace.root.join(&source.mount);
        if workspace
            .directories
            .iter()
            .any(|directory| directory.path == path)
        {
            return Err(format!("duplicate source mount: {:?}", source.mount));
        }
        let directory = WorkspaceDirectory {
            id: format!("{workspace_id}:{}", source.id),
            source_id: source.id.clone(),
            name: source.name.clone(),
            path,
            is_git: source.is_git,
            branch: None,
            effective_isolation: None,
            finished_head: None,
            status: DirectoryStatus::Pending,
            source_path: source.path.clone(),
            base_branch: source.base_branch.clone(),
            error: None,
        };
        workspace.directories.push(directory.clone());
        if let Err(error) = persist(workspace) {
            workspace.directories.pop();
            return Err(error);
        }
        Ok(directory)
    }

    /// Make one pending directory and write down what it became.
    ///
    /// An add that fails leaves no half-directory behind: the record goes back
    /// out and the error is the caller's. That is the difference from
    /// [`Self::provision`], which is retrying a workspace whose directories are
    /// all promised and must stay promised.
    pub fn materialize_directory<F>(
        &mut self,
        workspace_id: &str,
        directory_id: &str,
        materialize: F,
    ) -> Result<Workspace, String>
    where
        F: FnOnce(&WorkspaceDirectory) -> Result<MaterializedDirectory, String>,
    {
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        let pending = workspace
            .directories
            .iter()
            .find(|directory| directory.id == directory_id)
            .cloned()
            .ok_or_else(|| format!("unknown directory_id: {directory_id}"))?;
        let made = match materialize(&pending) {
            Ok(made) => made,
            Err(error) => {
                workspace
                    .directories
                    .retain(|directory| directory.id != directory_id);
                let _ = persist(workspace);
                return Err(error);
            }
        };
        let directory = workspace
            .directories
            .iter_mut()
            .find(|directory| directory.id == directory_id)
            .expect("the directory was just read");
        directory.branch = made.branch;
        directory.effective_isolation = Some(made.isolation);
        if let Some(path) = made.source_path {
            directory.source_path = path;
        }
        if let Some(base_branch) = made.base_branch {
            directory.base_branch = base_branch;
        }
        directory.status = DirectoryStatus::Ready;
        directory.error = None;
        persist(workspace)?;
        Ok(workspace.clone())
    }

    /// Drop one directory from a workspace's snapshot, its files having been
    /// removed and its checkout handed back.
    pub fn forget_directory(
        &mut self,
        workspace_id: &str,
        directory_id: &str,
    ) -> Result<Workspace, String> {
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        let before = workspace.directories.len();
        workspace
            .directories
            .retain(|directory| directory.id != directory_id);
        if workspace.directories.len() == before {
            return Err(format!("unknown directory_id: {directory_id}"));
        }
        persist(workspace)?;
        Ok(workspace.clone())
    }

    /// Write down that this workspace was finished, and what each of its
    /// repositories ended on.
    ///
    /// Done removes the workspace, so this record is all that is left of it:
    /// it goes beside the registry rather than inside the root that is about
    /// to be walked away. The Git work is measured once more first — the
    /// eligibility the caller checked was checked under the app mutex, and
    /// this runs after it was let go.
    pub fn record_finished(&self, workspace: &mut Workspace) -> Result<WorkspaceFinish, String> {
        let blockers = workspace_directory_blockers(workspace);
        if !blockers.is_empty() {
            return Err(finish_refusal(&blockers));
        }
        let repositories = workspace
            .directories
            .iter_mut()
            .filter(|directory| directory.is_git)
            .map(|directory| {
                let head = git_output(&directory.path, &["rev-parse", "HEAD"])?;
                directory.finished_head = Some(head);
                Ok(RepositoryFinish {
                    directory_id: directory.id.clone(),
                    pushed: true,
                    reason: None,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        workspace.status = WorkspaceStatus::Finished;
        workspace.archived_at = Some(crate::store::now_rfc3339());
        self.persist_finished_record(workspace)?;
        Ok(WorkspaceFinish {
            complete: true,
            repositories,
        })
    }

    /// Every workspace this registry has finished, read off the records kept
    /// outside the roots themselves. The files are gone, so these are the
    /// history of what was finished and where each source was left.
    pub fn finished_records(&self) -> Vec<Workspace> {
        let directory = self.root.join(".legacy");
        let Ok(entries) = fs::read_dir(&directory) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Workspace>(&bytes).ok())
            .collect()
    }

    pub fn retry_sources(workspace: &Workspace) -> Vec<WorkspaceSource> {
        workspace
            .directories
            .iter()
            .map(|directory| WorkspaceSource {
                id: directory.source_id.clone(),
                name: directory.name.clone(),
                mount: directory
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: directory.source_path.clone(),
                is_git: directory.is_git,
                base_branch: directory.base_branch.clone(),
            })
            .collect()
    }

    /// Register an existing Git-root checkout without moving or rewriting it.
    /// Legacy records are memory-only because the checkout is not owned by this
    /// registry and must remain untouched.
    pub fn adopt_root(
        &mut self,
        project_id: &str,
        id: String,
        name: String,
        path: PathBuf,
        source_id: String,
        is_git: bool,
    ) -> Workspace {
        if let Some(existing) = self.workspaces.get(&id) {
            return existing.clone();
        }
        let branch = is_git
            .then(|| git_output(&path, &["branch", "--show-current"]).ok())
            .flatten()
            .filter(|branch| !branch.is_empty());
        let proof = fs::read(self.root.join(".legacy").join(&id))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Workspace>(&bytes).ok());
        let status = if proof.is_some() {
            WorkspaceStatus::Finished
        } else {
            WorkspaceStatus::Ready
        };
        let mut workspace = Workspace {
            id: id.clone(),
            project_id: project_id.to_string(),
            name: name.clone(),
            root: path.clone(),
            status,
            archived_at: proof.as_ref().and_then(|proof| proof.archived_at.clone()),
            directories: vec![WorkspaceDirectory {
                id: format!("{id}:root"),
                source_id,
                name,
                path,
                is_git,
                branch,
                effective_isolation: None,
                finished_head: None,
                status: DirectoryStatus::Ready,
                source_path: PathBuf::new(),
                base_branch: String::new(),
                error: None,
            }],
            isolation: Isolation::default(),
            managed: false,
            created_by_agent: false,
        };
        if let Some(proof) = proof {
            workspace.directories[0].finished_head = proof
                .directories
                .first()
                .and_then(|directory| directory.finished_head.clone());
        }
        self.workspaces.insert(id, workspace.clone());
        workspace
    }

    /// The one record of a finished workspace that survives it: a copy of the
    /// manifest under the registry root, where removing the workspace's own
    /// directory cannot take it.
    fn persist_finished_record(&self, workspace: &Workspace) -> Result<(), String> {
        let directory = self.root.join(".legacy");
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "create finished workspace records {}: {error}",
                directory.display()
            )
        })?;
        let path = directory.join(&workspace.id);
        let bytes = serde_json::to_vec(workspace)
            .map_err(|error| format!("serialize finished workspace {}: {error}", workspace.id))?;
        atomic_write(&path, &bytes).map_err(|error| {
            format!(
                "write finished workspace record {}: {error}",
                path.display()
            )
        })
    }

    /// Compatibility spelling for callers that only adopt Git roots.
    pub fn adopt_git_root(
        &mut self,
        project_id: &str,
        id: String,
        name: String,
        path: PathBuf,
        source_id: String,
    ) -> Workspace {
        self.adopt_root(project_id, id, name, path, source_id, true)
    }
}

/// Why Done is not offered on a workspace. A row spells these as its
/// `finish_blockers`, and the verb refuses in the same words, so what the
/// button says and what the bridge does cannot drift.
pub const FINISH_BLOCKER_AGENT_WORKING: &str = "agent_working";
pub const FINISH_BLOCKER_DIRTY: &str = "dirty";
pub const FINISH_BLOCKER_UNPUSHED: &str = "unpushed";
/// Nothing has managed to read this workspace's Git state, so Done cannot say
/// the work is safe anywhere else.
pub const FINISH_BLOCKER_UNKNOWN: &str = "unknown";
/// The workspace holds an ordinary directory. It has no published baseline, so
/// nothing can say its files are anywhere else — and Done removes them.
pub const FINISH_BLOCKER_PLAIN_DIRECTORY: &str = "plain_directory";

/// What a Git directory's work summary says about Done, in the order a reader
/// meets them. Empty means this directory has nothing left to lose: every
/// commit is in its push destination and the tree is clean.
pub fn summary_finish_blockers(summary: &crate::gitgui::WorkSummary) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if summary.dirty {
        blockers.push(FINISH_BLOCKER_DIRTY);
    }
    if summary.pushes > 0 {
        blockers.push(FINISH_BLOCKER_UNPUSHED);
    }
    blockers
}

/// What stands between this workspace's directories and Done, measured now
/// rather than read off the feed's cache.
///
/// An ordinary directory is a blocker in itself: no remote holds a copy of it
/// and nothing measures whether it was edited, so Done — which removes the
/// workspace — cannot say its files are anywhere else. A Git directory is
/// measured, and a workspace holding no directory at all has nothing to lose.
pub fn workspace_directory_blockers(workspace: &Workspace) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if workspace.directories.iter().any(|d| !d.is_git) {
        blockers.push(FINISH_BLOCKER_PLAIN_DIRECTORY);
    }
    for directory in workspace.directories.iter().filter(|d| d.is_git) {
        let measured = crate::gitgui::work_summary(&directory.path)
            .map(|summary| summary_finish_blockers(&summary))
            .unwrap_or_else(|_| vec![FINISH_BLOCKER_UNKNOWN]);
        for blocker in measured {
            if !blockers.contains(&blocker) {
                blockers.push(blocker);
            }
        }
    }
    blockers.sort_unstable_by_key(|blocker| blocker_order(blocker));
    blockers
}

/// One order for the whole list, whoever assembled it: the live reason first,
/// then what is on disk, then what is only local, then not knowing.
pub fn blocker_order(blocker: &str) -> usize {
    match blocker {
        FINISH_BLOCKER_AGENT_WORKING => 0,
        FINISH_BLOCKER_DIRTY => 1,
        FINISH_BLOCKER_UNPUSHED => 2,
        FINISH_BLOCKER_PLAIN_DIRECTORY => 3,
        _ => 4,
    }
}

/// How a blocker reads in a sentence, for the one refusal Done ever gives.
pub fn blocker_sentence(blocker: &str) -> &'static str {
    match blocker {
        FINISH_BLOCKER_AGENT_WORKING => "an agent is working in it",
        FINISH_BLOCKER_DIRTY => "it has uncommitted changes",
        FINISH_BLOCKER_UNPUSHED => "it has commits no remote has",
        FINISH_BLOCKER_PLAIN_DIRECTORY => "it holds a directory that is not a repository",
        _ => "its Git state could not be read",
    }
}

/// Done's refusal, in the words the row's blockers are spelled in and a
/// sentence saying each one. Done removes the workspace, so a refusal is the
/// whole of what protects work that is nowhere else yet.
pub fn finish_refusal(blockers: &[&str]) -> String {
    format!(
        "workspace.finish is not available yet: {}",
        blockers
            .iter()
            .map(|blocker| blocker_sentence(blocker))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

fn validate_loaded_workspace(
    workspace: &Workspace,
    manifest_parent: &Path,
    project_parent: &Path,
    registry_root: &Path,
) -> Result<(), String> {
    let actual_root = fs::canonicalize(manifest_parent).map_err(|error| {
        format!(
            "resolve workspace root {}: {error}",
            manifest_parent.display()
        )
    })?;
    let recorded_root = fs::canonicalize(&workspace.root).map_err(|error| {
        format!(
            "resolve recorded workspace root {}: {error}",
            workspace.root.display()
        )
    })?;
    if !actual_root.starts_with(registry_root)
        || actual_root != recorded_root
        || workspace.project_id
            != project_parent
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
    {
        return Err(format!(
            "workspace manifest {} does not describe its containing directory",
            manifest_parent.display()
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for directory in &workspace.directories {
        if !ids.insert(&directory.id)
            || directory.path.parent() != Some(actual_root.as_path())
            || directory
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| validate_segment("source mount", name).is_err())
        {
            return Err(format!(
                "workspace {} contains an invalid directory record",
                workspace.id
            ));
        }
        if directory.path.exists() {
            let resolved = fs::canonicalize(&directory.path).map_err(|error| {
                format!(
                    "resolve workspace directory {}: {error}",
                    directory.path.display()
                )
            })?;
            if !resolved.starts_with(&actual_root) {
                return Err(format!(
                    "workspace directory {} escapes its root",
                    directory.path.display()
                ));
            }
        }
    }
    Ok(())
}

fn git_output(path: &Path, args: &[&str]) -> Result<String, String> {
    crate::git_process::run_git(path, args)
        .map(|output| output.trim().to_string())
        .map_err(|error| error.to_string())
}

fn persist(workspace: &Workspace) -> Result<(), String> {
    let manifest = workspace.root.join(MANIFEST_FILE);
    let bytes = serde_json::to_vec_pretty(workspace)
        .map_err(|error| format!("serialize workspace {}: {error}", workspace.id))?;
    atomic_write(&manifest, &bytes)
        .map_err(|error| format!("publish workspace manifest {}: {error}", manifest.display()))?;
    // Once the durable manifest exists it carries the same checkout identity.
    // Marker cleanup is cosmetic and must not turn a published success into a
    // reported failure.
    let _ = fs::remove_file(workspace.root.join(PENDING_MARKER_FILE));
    Ok(())
}

fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let temporary = parent.join(format!(".{name}.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn read_dirs(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
    fs::read_dir(path)
        .map_err(|error| format!("read workspaces {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read workspaces {}: {error}", path.display()))
}

fn validate_segment(kind: &str, segment: &str) -> Result<(), String> {
    let path = Path::new(segment);
    if segment.trim().is_empty() || path.components().count() != 1 || matches!(segment, "." | "..")
    {
        return Err(format!("invalid {kind}: {segment:?}"));
    }
    Ok(())
}

fn create_unique_dir(parent: &Path, requested: &str) -> Result<PathBuf, String> {
    for suffix in 1_u64.. {
        let candidate = if suffix == 1 {
            parent.join(requested)
        } else {
            parent.join(format!("{requested}-{suffix}"))
        };
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create workspace {}: {error}", candidate.display())),
        }
    }
    unreachable!("an unbounded suffix sequence has an unused path")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &Path, id: &str, mount: &str) -> WorkspaceSource {
        WorkspaceSource {
            id: id.to_string(),
            name: mount.to_string(),
            mount: mount.to_string(),
            path: root.join(format!("source-{id}")),
            is_git: false,
            base_branch: "main".to_string(),
        }
    }

    #[test]
    fn free_form_workspace_name_is_preserved_while_its_path_is_safe_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![source(temp.path(), "one", "one")];
        let mut registry = WorkspaceRegistry::load(temp.path().join("workspaces")).unwrap();
        let name = format!("  A / surprising 🦀 workspace .. {}  ", "x".repeat(200));

        let workspace = registry.begin("project", &name, &sources).unwrap();

        assert_eq!(workspace.name, name);
        let directory_name = workspace.root.file_name().unwrap().to_string_lossy();
        assert!(directory_name.len() <= 50, "{directory_name}");
        assert!(directory_name
            .chars()
            .all(|character| character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'));
        assert!(workspace
            .root
            .starts_with(temp.path().join("workspaces/project")));
    }

    #[test]
    fn empty_and_whitespace_workspace_names_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![source(temp.path(), "one", "one")];
        let mut registry = WorkspaceRegistry::load(temp.path().join("workspaces")).unwrap();

        let empty = registry.begin("project", "", &sources).unwrap();
        let spaces = registry.begin("project", "   ", &sources).unwrap();

        assert_eq!(empty.name, "");
        assert_eq!(spaces.name, "   ");
        assert_eq!(empty.root.file_name().unwrap(), "task");
        assert_eq!(spaces.root.file_name().unwrap(), "task-2");
    }

    #[test]
    fn interrupted_provisioning_recovers_and_resumes_only_unfinished_sources() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![
            source(temp.path(), "one", "one"),
            source(temp.path(), "two", "two"),
        ];
        let mut registry = WorkspaceRegistry::load(temp.path().join("workspaces")).unwrap();
        let workspace = registry.begin("project", "feature", &sources).unwrap();
        let error = registry
            .provision(
                &workspace.id,
                &sources,
                Isolation::Worktree,
                |source, path, _| {
                    if source.id == "two" {
                        return Err("injected copy failure".to_string());
                    }
                    fs::create_dir(path).map_err(|error| error.to_string())?;
                    Ok((None, Isolation::Worktree))
                },
            )
            .unwrap_err();
        assert_eq!(error, "injected copy failure");

        let mut recovered = WorkspaceRegistry::load(temp.path().join("workspaces")).unwrap();
        let mut copied = Vec::new();
        let resumed = recovered
            .provision(
                &workspace.id,
                &sources,
                Isolation::Worktree,
                |source, path, _| {
                    copied.push(source.id.clone());
                    fs::create_dir(path).map_err(|error| error.to_string())?;
                    Ok((None, Isolation::Worktree))
                },
            )
            .unwrap();
        assert_eq!(copied, vec!["two"]);
        assert_eq!(resumed.status, WorkspaceStatus::Ready);
    }

    #[test]
    fn adopts_git_root_without_writing_a_manifest() {
        let temp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .arg(temp.path())
            .status()
            .unwrap();
        let mut registry = WorkspaceRegistry::empty(temp.path().join("managed"));
        let workspace = registry.adopt_root(
            "project",
            "legacy".to_string(),
            "repo".to_string(),
            temp.path().to_path_buf(),
            "source".to_string(),
            true,
        );
        assert_eq!(workspace.directories[0].path, temp.path());
        assert!(!temp.path().join(MANIFEST_FILE).exists());
    }

    #[test]
    fn live_reload_does_not_interrupt_an_active_provisioning_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![source(temp.path(), "one", "one")];
        let root = temp.path().join("workspaces");
        let mut registry = WorkspaceRegistry::load(&root).unwrap();
        let workspace = registry.begin("project", "active", &sources).unwrap();

        let reloaded = WorkspaceRegistry::load(&root).unwrap();
        assert_eq!(
            reloaded.get(&workspace.id).unwrap().status,
            WorkspaceStatus::Provisioning
        );
        assert_eq!(
            reloaded.get(&workspace.id).unwrap().directories[0].status,
            DirectoryStatus::Pending
        );
    }

    #[test]
    fn startup_recovery_marks_orphaned_provisioning_as_retryable() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![source(temp.path(), "one", "one")];
        let root = temp.path().join("workspaces");
        let mut registry = WorkspaceRegistry::load(&root).unwrap();
        let workspace = registry.begin("project", "interrupted", &sources).unwrap();

        let recovered = WorkspaceRegistry::recover(&root).unwrap();
        let recovered = recovered.get(&workspace.id).unwrap();
        assert_eq!(recovered.status, WorkspaceStatus::Failed);
        assert_eq!(recovered.directories[0].status, DirectoryStatus::Pending);
        assert!(recovered.directories[0]
            .error
            .as_deref()
            .unwrap()
            .contains("retry"));
    }

    #[test]
    fn retry_claim_is_persisted_and_rejects_a_concurrent_retry() {
        let temp = tempfile::tempdir().unwrap();
        let sources = vec![source(temp.path(), "one", "one")];
        let root = temp.path().join("workspaces");
        let mut registry = WorkspaceRegistry::load(&root).unwrap();
        let workspace = registry.begin("project", "failed", &sources).unwrap();
        let failed = registry.workspaces.get_mut(&workspace.id).unwrap();
        failed.status = WorkspaceStatus::Failed;
        persist(failed).unwrap();

        let claimed = registry.claim_retry(&workspace.id).unwrap();
        assert_eq!(claimed.status, WorkspaceStatus::Provisioning);
        assert!(registry
            .claim_retry(&workspace.id)
            .unwrap_err()
            .contains("already running"));
        assert_eq!(
            WorkspaceRegistry::load(&root)
                .unwrap()
                .get(&workspace.id)
                .unwrap()
                .status,
            WorkspaceStatus::Provisioning
        );
    }
}
