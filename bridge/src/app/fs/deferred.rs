//! Filesystem preparation, timed record validation, service, and memory publication.
use super::path_lock::DestinationLock;
use super::{AppState, FileScope, MOVED_SOURCE};
use crate::api::ApiError;
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{DeferredGit, DeferredWork, TermScope};
use crate::source_sync::SyncLock;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type Work<T> = Box<dyn FnOnce() -> T + Send>;
type Apply<T> = Box<dyn FnOnce(&mut AppState, T) -> Result<Value, ApiError> + Send>;

struct FileJob<T> {
    work: Mutex<Option<Work<T>>>,
    result: Mutex<Option<T>>,
    apply: Mutex<Option<Apply<T>>>,
}

impl<T: Send + 'static> DeferredGitWork for FileJob<T> {
    fn run(&self, _params: &Value) -> Result<Value, String> {
        let work = self.work.lock().unwrap().take().expect("one file service");
        *self.result.lock().unwrap() = Some(work());
        Ok(Value::Null)
    }

    fn invalidate(&self, _app: &mut AppState) {}

    fn settle(&self, app: &mut AppState, _answer: Value) -> Result<Value, String> {
        let result = self.result.lock().unwrap().take().expect("file result");
        let apply = self
            .apply
            .lock()
            .unwrap()
            .take()
            .expect("one file publication");
        apply(app, result).map_err(encode_error)
    }
}

pub(super) fn encode_error(error: ApiError) -> String {
    super::errors::encode(error)
}

impl AppState {
    pub(super) fn defer_file<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
        apply: impl FnOnce(&mut AppState, T) -> Result<Value, ApiError> + Send + 'static,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(FileJob {
                work: Mutex::new(Some(Box::new(work))),
                result: Mutex::new(None),
                apply: Mutex::new(Some(Box::new(apply))),
            }),
            params: Value::Null,
            invalidates: false,
            #[cfg(test)]
            gate: None,
        })));
        Value::Null
    }
}

/// Stable ids and their recorded path; canonicalization belongs to preparation.
#[derive(Clone)]
pub(super) struct FileTarget {
    pub(super) scope: Option<FileScope>,
    pub(super) recorded_root: PathBuf,
    pub(super) destination: Option<String>,
}

impl FileTarget {
    pub(super) fn capture(app: &AppState, scope: FileScope) -> Result<Self, ApiError> {
        let recorded_root = scope.recorded_root(app).map_err(ApiError::classify)?;
        Ok(Self {
            scope: Some(scope),
            recorded_root,
            destination: None,
        })
    }

    pub(super) fn host(parent: PathBuf) -> Self {
        Self {
            scope: None,
            recorded_root: parent,
            destination: None,
        }
    }

    pub(super) fn prepare(&self) -> Result<PreparedFile, ApiError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let root = std::fs::canonicalize(&self.recorded_root).map_err(|error| {
            ApiError::invalid_params(format!("cannot resolve scope root: {error}"))
        })?;
        let repository = SyncLock::acquire_many(
            std::slice::from_ref(&root),
            deadline.saturating_duration_since(Instant::now()),
        )
        .map_err(|_| ApiError::busy("Another filesystem operation is still running; try again."))?;
        let destination = self
            .destination
            .as_ref()
            .map(|relative| DestinationLock::acquire(&root, relative, deadline))
            .transpose()?;
        let held = FileLocks {
            _repository: repository,
            _destination: destination,
        };
        let current = std::fs::canonicalize(&self.recorded_root).map_err(|error| {
            ApiError::invalid_params(format!("cannot resolve scope root: {error}"))
        })?;
        if current != root {
            return Err(ApiError::conflict(MOVED_SOURCE, None));
        }
        Ok(PreparedFile { root, held })
    }

    pub(super) fn validate(&self, app: &AppState, root: &Path) -> Result<(), ApiError> {
        if let Some(scope) = &self.scope {
            let current = scope.recorded_root(app).map_err(ApiError::classify)?;
            if current != self.recorded_root {
                return Err(ApiError::conflict(MOVED_SOURCE, None));
            }
        }
        // Roots have already been canonicalized off-lock; reservation membership
        // is an in-memory containment check, without another filesystem probe.
        if app.reclaim_reserved_at(root) {
            return Err(ApiError::busy(crate::reclaim::RESERVED));
        }
        Ok(())
    }
}

pub(super) struct PreparedFile {
    pub(super) root: PathBuf,
    // Keep repository serialization through the final memory publication.
    pub(super) held: FileLocks,
}

pub(super) struct FileLocks {
    _repository: Vec<SyncLock>,
    _destination: Option<DestinationLock>,
}

/// Disk changes can precede an error response: readback or reopen persistence
/// may fail, or publication may refuse a moved source or a new reservation.
/// Those failures do not roll back bytes already written. Closing the general
/// service-to-publication window belongs to step 6b's acknowledgement and
/// confirmation work.
pub(super) struct FileMutation {
    pub(super) answer: Result<Value, ApiError>,
    pub(super) changed: bool,
}

impl AppState {
    /// The first trip performs path and lock work. The timed apply guard only
    /// checks captured records before handing the service its held source lock.
    pub(super) fn prepare_file(
        &mut self,
        target: FileTarget,
        apply: impl FnOnce(
                &mut AppState,
                FileTarget,
                Result<PreparedFile, ApiError>,
            ) -> Result<Value, ApiError>
            + Send
            + 'static,
    ) -> Value {
        let captured = target.clone();
        self.defer_file(
            move || captured.prepare(),
            move |app, prepared| apply(app, target, prepared),
        )
    }

    pub(super) fn file_reopen(
        &self,
        target: &FileTarget,
    ) -> Result<Option<crate::workspace::FileReopen>, ApiError> {
        let Some(FileScope::WorkspaceSource { workspace_id, .. }) = &target.scope else {
            return Ok(None);
        };
        self.workspaces
            .prepare_file_reopen(workspace_id)
            .map_err(ApiError::classify)
    }

    pub(super) fn publish_file_mutation(
        &mut self,
        target: &FileTarget,
        root: &Path,
        reopened: Result<Option<crate::workspace::FileReopen>, ApiError>,
    ) -> Result<(), ApiError> {
        if target.scope.is_none() {
            return target.validate(self, root);
        }
        self.invalidate_snapshot_checkout_observations(root);
        let reopened = reopened?;
        target.validate(self, root)?;
        if let Some(reopened) = reopened {
            self.workspaces
                .apply_file_reopen(reopened)
                .map_err(ApiError::classify)?;
        }
        if let Some(scope) = &target.scope {
            self.invalidate_file_scope(scope);
        }
        Ok(())
    }
}

impl FileScope {
    /// Resolve from installed records only. Reads retain the legacy resolver;
    /// mutations must not adopt manifests or canonicalize while holding app.
    fn recorded_root(&self, app: &AppState) -> Result<PathBuf, String> {
        match self {
            Self::WorkspaceSource {
                workspace_id,
                source_id,
            } => {
                let workspace = app
                    .workspaces
                    .get(workspace_id)
                    .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
                workspace
                    .directories
                    .iter()
                    .find(|directory| {
                        directory.source_id == *source_id || directory.id == *source_id
                    })
                    .map(|directory| directory.path.clone())
                    .ok_or_else(|| {
                        format!("unknown source_id {source_id} in workspace {workspace_id}")
                    })
            }
            Self::ProjectSource {
                project_id,
                source_id,
                source_path,
            } => project_source_path(app, project_id, source_id, source_path.as_deref()),
            Self::Legacy(scope) => legacy_root(app, scope),
        }
    }
}

fn project_source_path(
    app: &AppState,
    project_id: &str,
    source_id: &str,
    opened: Option<&str>,
) -> Result<PathBuf, String> {
    let project = app
        .projects
        .get(project_id)
        .ok_or_else(|| format!("unknown project_id: {project_id}"))?;
    let source = project
        .sources
        .iter()
        .find(|source| source.id == source_id)
        .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
    if opened.is_some_and(|opened| opened != source.path.to_string_lossy().as_ref()) {
        return Err(MOVED_SOURCE.into());
    }
    Ok(source.path.clone())
}

fn legacy_root(app: &AppState, scope: &TermScope) -> Result<PathBuf, String> {
    match scope {
        TermScope::Run { run_id } => {
            let run = app.runs.get(run_id).ok_or("unknown run_id")?;
            Ok(app
                .workspaces
                .get(run_id)
                .or_else(|| {
                    app.workspaces
                        .iter()
                        .find(|workspace| workspace.root == run.worktree.path)
                })
                .and_then(|workspace| {
                    workspace
                        .directories
                        .iter()
                        .find(|directory| directory.is_git)
                })
                .map(|directory| directory.path.clone())
                .unwrap_or_else(|| run.worktree.path.clone()))
        }
        TermScope::Primary { project_id } => app
            .projects
            .get(project_id)
            .map(|project| project.repo_path.clone())
            .ok_or_else(|| "unknown project_id".into()),
        TermScope::ExternalWorktree {
            project_id,
            worktree_id,
        } => app
            .board
            .diff()
            .external_scan(project_id)
            .worktrees
            .iter()
            .find(|worktree| worktree.id == *worktree_id)
            .map(|worktree| worktree.path.clone())
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}")),
        TermScope::ProjectSource(source) => project_source_path(
            app,
            &source.project_id,
            &source.source_id,
            Some(&source.source_path),
        ),
    }
}

impl AppState {
    pub(super) fn file_mutation(
        &mut self,
        target: FileTarget,
        work: impl FnOnce(&Path) -> Result<Value, ApiError> + Send + 'static,
    ) -> Value {
        self.file_mutation_result(target, move |root| {
            let answer = work(root);
            let changed = answer.is_ok();
            FileMutation { answer, changed }
        })
    }

    pub(super) fn file_mutation_result(
        &mut self,
        target: FileTarget,
        work: impl FnOnce(&Path) -> FileMutation + Send + 'static,
    ) -> Value {
        self.prepare_file(target, move |app, target, prepared| {
            let prepared = prepared?;
            target.validate(app, &prepared.root)?;
            let reopened = app.file_reopen(&target)?;
            let root = prepared.root.clone();
            Ok(app.defer_file(
                move || {
                    #[cfg(test)]
                    super::service_test::service();
                    let change = work(&prepared.root);
                    let reopened = if change.changed {
                        persist_reopen(reopened)
                    } else {
                        Ok(None)
                    };
                    (change, reopened, prepared.held)
                },
                move |app, (change, reopened, _held)| {
                    if change.changed {
                        app.publish_file_mutation(&target, &root, reopened)?;
                    }
                    change.answer
                },
            ))
        })
    }
}

pub(super) fn persist_reopen(
    reopened: Option<crate::workspace::FileReopen>,
) -> Result<Option<crate::workspace::FileReopen>, ApiError> {
    if let Some(reopened) = &reopened {
        reopened.persist().map_err(ApiError::classify)?;
    }
    Ok(reopened)
}
