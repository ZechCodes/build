//! Saved creation reservations use the ordinary unpublished workspace worker.
use super::*;
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static PREPARATION_FAILURE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(in crate::app) fn fail_next_preparation_for_test(error: &str) {
    PREPARATION_FAILURE.with_borrow_mut(|saved| *saved = Some(error.into()));
}
#[cfg(test)]
pub(super) fn take_preparation_failure() -> Option<String> {
    PREPARATION_FAILURE.with_borrow_mut(Option::take)
}

pub(in crate::app) enum WorkspaceCreationEvent {
    #[cfg(test)]
    MaterializedBeforeCheckpoint,
    Started,
    RolledBack {
        error: String,
    },
    Step {
        step: String,
        state: String,
        detail: Option<String>,
    },
    Checkpoint(Workspace),
    Residue {
        workspace: Workspace,
        error: String,
    },
}
pub(in crate::app) type WorkspaceCreationObserver =
    Arc<dyn Fn(WorkspaceCreationEvent) -> Result<(), String> + Send + Sync>;

impl AppState {
    pub(in crate::app) fn reserve_creation_workspace(
        &self,
        project_id: &str,
        params: &Value,
    ) -> Result<Workspace, String> {
        let sources = self
            .sources_for(project_id)?
            .into_iter()
            .map(|s| WorkspaceSource {
                id: s.id,
                name: s.name,
                mount: s.mount,
                path: s.path,
                is_git: s.is_git,
                base_branch: s.base_branch,
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return Err("workspace.create: project has no sources".into());
        }
        self.workspace_secret_source_policy().sources(&sources)?;
        let isolation = params
            .get("isolation")
            .and_then(Value::as_str)
            .map(|v| Isolation::from_wire(v).ok_or_else(|| format!("unknown isolation: {v}")))
            .transpose()?
            .unwrap_or_else(|| {
                self.project_isolation(self.projects.get(project_id).expect("resolved project"))
            });
        let mut workspace = self.workspaces.reserve_delivery_workspace(
            project_id,
            params["name"].as_str().unwrap_or("workspace"),
            &sources,
            isolation,
        )?;
        workspace.created_by_agent = params["made_by_agent"].as_bool().unwrap_or(false);
        Ok(workspace)
    }
    pub(in crate::app) fn prepare_creation_workspace(
        &mut self,
        reserved: &Workspace,
        observer: Option<WorkspaceCreationObserver>,
    ) -> Result<DeferredGit, String> {
        self.mark_device_lifecycle_dirty()?;
        self.project_for(&reserved.project_id)?;
        if self.workspace_reclaim_reserved(&reserved.id) {
            return Err(crate::reclaim::BUSY.into());
        }
        let pending = WorkspaceRegistry::retry_sources(reserved);
        let sources = reserved
            .directories
            .iter()
            .map(|d| WorkspaceSource {
                id: d.source_id.clone(),
                name: d.name.clone(),
                mount: d
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: d.source_path.clone(),
                is_git: d.is_git,
                base_branch: d.base_branch.clone(),
            })
            .collect::<Vec<_>>();
        let secret_sources = self.workspace_secret_source_policy();
        secret_sources.workspace(reserved)?;
        Ok(DeferredGit {
            call: Box::new(WorkspaceCreateWork {
                registry_root: self.workspaces.root().to_path_buf(),
                rift_root: self.project_worktrees_root(&reserved.project_id),
                workspace_id: reserved.id.clone(),
                workspace_name: reserved
                    .root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                workspace: reserved.clone(),
                unpublished: true,
                recover_existing: true,
                base_syncs: self.retry_sync_subjects(&reserved.project_id, &pending),
                sources,
                secret_sources,
                isolation: reserved.isolation,
                source_state: self.source_validation_handle(),
                synced: Default::default(),
                observer,
                #[cfg(test)]
                prepared_gate: None,
            }),
            params: Value::Null,
            invalidates: true,
            registry_change: true,
            #[cfg(test)]
            gate: None,
        })
    }
    pub(in crate::app) fn prepare_creation_cleanup(
        &mut self,
        reserved: &Workspace,
    ) -> Result<DeferredGit, String> {
        let mut work = self.prepare_creation_workspace(reserved, None)?;
        work.call = Box::new(CleanupCreation {
            work: work.call,
            registry_root: self.workspaces.root().to_path_buf(),
            reserved: reserved.clone(),
            rift_root: self.project_worktrees_root(&reserved.project_id),
        });
        Ok(work)
    }
}

struct CleanupCreation {
    work: Box<dyn DeferredGitWork>,
    registry_root: PathBuf,
    reserved: Workspace,
    rift_root: PathBuf,
}
impl DeferredGitWork for CleanupCreation {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let registry = WorkspaceRegistry::load(&self.registry_root)?;
        registry.validate_creation_cleanup(&self.reserved)?;
        cleanup_directories(&self.reserved, &self.rift_root)?;
        match std::fs::remove_dir_all(&self.reserved.root) {
            Ok(()) => Ok(Value::Null),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
            Err(e) => Err(e.to_string()),
        }
    }
    fn invalidate(&self, app: &mut AppState) {
        self.work.invalidate(app);
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
}
pub(super) fn cleanup_directories(workspace: &Workspace, rift_root: &Path) -> Result<(), String> {
    for directory in workspace.directories.iter().rev() {
        if let Some(branch) = &directory.branch {
            WorktreeManager::new(
                &directory.source_path,
                directory.path.parent().unwrap_or(&directory.path),
            )
            .with_rift_registry_root(rift_root)
            .remove(&Worktree {
                name: directory
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: directory.path.clone(),
                recorded_branch: branch.clone(),
                base_branch: directory.base_branch.clone(),
            })
            .map_err(|e| e.to_string())?;
            continue;
        }
        if !directory.path.exists() {
            continue;
        }
        if directory.is_git {
            WorktreeManager::new(
                &directory.source_path,
                directory.path.parent().unwrap_or(&directory.path),
            )
            .with_rift_registry_root(rift_root)
            .remove_checkout(&directory.path)
            .map_err(|e| e.to_string())?;
        } else {
            remove_directory_with_rift_root(
                &directory.source_path,
                &directory.path,
                directory.effective_isolation.unwrap_or(workspace.isolation),
                rift_root,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn journal_workspace_reservation_is_pure_and_reports_directory_progress() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("data"), b"saved").unwrap();
        let mut app = AppState::new_unrooted(
            temp.path().join("workspaces"),
            "main",
            true,
            temp.path().join("mcp.sock").display().to_string(),
        );
        let project = app.add_project(source, "main".into());
        let reserved = app
            .reserve_creation_workspace(&project, &json!({"name":"first"}))
            .unwrap();
        assert!(!reserved.root.exists());
        let events = Arc::new(Mutex::new(Vec::new()));
        let saved_events = events.clone();
        let observer: WorkspaceCreationObserver = Arc::new(move |event| {
            saved_events.lock().unwrap().push(event);
            Ok(())
        });
        let work = app
            .prepare_creation_workspace(&reserved, Some(observer))
            .unwrap();
        let result = work.run().unwrap();
        assert_eq!(result["workspace_id"], reserved.id);
        let events = events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(event, WorkspaceCreationEvent::Step { state, .. } if state == "running")));
        assert!(events.iter().any(
            |event| matches!(event, WorkspaceCreationEvent::Step { state, .. } if state == "done")
        ));
        assert!(events.iter().any(|event| matches!(event, WorkspaceCreationEvent::Checkpoint(workspace) if workspace.directories[0].status == crate::workspace::DirectoryStatus::Ready)));
    }

    #[test]
    fn creation_cleanup_refuses_unowned_root() {
        let temp = tempfile::tempdir().unwrap();
        let registry = WorkspaceRegistry::empty(temp.path().join("workspaces"));
        let reserved = registry
            .reserve_delivery_workspace("project", "first", &[], Isolation::Worktree)
            .unwrap();
        std::fs::create_dir_all(&reserved.root).unwrap();
        std::fs::write(reserved.root.join("foreign"), b"keep").unwrap();
        assert!(registry.validate_creation_cleanup(&reserved).is_err());
        assert!(reserved.root.join("foreign").exists());
    }
}
