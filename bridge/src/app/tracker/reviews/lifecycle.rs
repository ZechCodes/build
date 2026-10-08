//! Explicit PR lifecycle; observation and Git run with the app lock released.
use super::super::StoredAnswer;
use super::projection;
use crate::api::v1::reviews::{errors, ReviewCloseParams, ReviewVersionParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::changes::{BoardLists, Kind};
use crate::reviews::lifecycle::{ReopenHooks, ReopenRequest};
use crate::reviews::model::{PullRequestStatus, ReviewMode};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::{Actor, Task};
use crate::workspace::{Workspace, MANIFEST_FILE};
use serde_json::{json, Value};
use std::sync::{Mutex, Weak};

pub(super) fn version(review: &Review, expected: u64) -> Result<(), String> {
    if review.version != expected {
        return Err(errors::encode(
            "stale_version",
            "The review changed; fetch its current version",
            json!({
                "task_id":review.task_id,"expected_version":expected,"current_version":review.version,
                "recovery":"Read tasks.review.get before retrying."
            }),
        ));
    }
    Ok(())
}

pub(super) fn invalidates(app: &AppState, project: &str, task: &str, workspace: &str) {
    app.note_tasks_changed(project, task);
    app.changes.note_kind(workspace, Kind::State);
    app.changes.note_kind(workspace, Kind::Git);
    app.changes.note_board_lists(BoardLists::WORKSPACES);
}

enum SyncOperation {
    Refresh {
        task_id: String,
        expected_version: u64,
        actor: Actor,
    },
    Reopen(ReopenRequest),
}

struct SyncJob {
    store: Store,
    project_id: String,
    workspace_id: String,
    operation: SyncOperation,
    hooks: WorkspaceHooks,
}

impl SyncJob {
    fn task_id(&self) -> &str {
        match &self.operation {
            SyncOperation::Refresh { task_id, .. } => task_id,
            SyncOperation::Reopen(request) => &request.task_id,
        }
    }

    fn synchronize(&self) -> Result<Review, String> {
        match &self.operation {
            SyncOperation::Refresh {
                task_id,
                expected_version,
                actor,
            } => {
                let review = self
                    .store
                    .load_review(task_id)
                    .stored()?
                    .ok_or("unknown PR review")?;
                version(&review, *expected_version)?;
                crate::reviews::sync::reconcile::reconcile_as(&self.store, task_id, actor.clone())?;
                self.store
                    .load_review(task_id)
                    .stored()?
                    .ok_or_else(|| "unknown PR review".into())
            }
            SyncOperation::Reopen(request) => {
                let review = self
                    .store
                    .load_review_sync_state(&request.task_id)
                    .stored()?
                    .ok_or("unknown PR review")?;
                version(&review, request.expected_version)?;
                crate::reviews::lifecycle::reopen(&self.store, request, &self.hooks)
            }
        }
    }
}

impl DeferredGitWork for SyncJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        self.synchronize().and_then(|review| projection::result(&self.store, review))
            .map_err(|message| errors::service(message, json!({
                "task_id":self.task_id(),"workspace_id":self.workspace_id,
                "recovery":"Read tasks.review.get; restore the recorded bindings before reopening."
            })))
    }

    fn invalidate(&self, app: &mut AppState) {
        invalidates(app, &self.project_id, self.task_id(), &self.workspace_id);
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
}

/// The deferred claim holds registry mutations until this job settles. Shared
/// hooks also check live reclaim/placement at every service checkpoint.
struct WorkspaceHooks {
    state: Option<Weak<Mutex<AppState>>>,
}

impl ReopenHooks for WorkspaceHooks {
    fn check_workspace(&self, workspace: &Workspace) -> Result<(), String> {
        if let Some(state) = self.state.as_ref().and_then(Weak::upgrade) {
            let app = state.lock().map_err(|_| "PR workspace state unavailable")?;
            app.refuse_writers_while_reserved(&workspace.root)?;
            let current = app
                .workspaces
                .get(&workspace.id)
                .ok_or("restore the original review workspace before reopening")?;
            check_placement(current, workspace)?;
            for directory in &workspace.directories {
                app.refuse_writers_while_reserved(&directory.path)?;
            }
        }
        let bytes =
            std::fs::read(workspace.root.join(MANIFEST_FILE)).map_err(|error| error.to_string())?;
        let saved: Workspace = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        check_placement(&saved, workspace)
    }
}

fn check_placement(current: &Workspace, expected: &Workspace) -> Result<(), String> {
    if current.id != expected.id
        || current.root != expected.root
        || current.project_id != expected.project_id
        || current.directories != expected.directories
        || !current.managed
    {
        return Err("conflict: original PR workspace identity or placement changed".into());
    }
    Ok(())
}

impl AppState {
    pub(super) fn pull_request(&mut self, task_id: &str) -> Result<(String, Task, Review), String> {
        let (project_id, task) = self.tracker_task(task_id)?;
        let review = self
            .tracker_store()?
            .load_review(task_id)
            .stored()?
            .ok_or_else(|| format!("unknown review for task_id: {task_id}"))?;
        if review.mode != ReviewMode::PullRequest || review.pull_request.is_none() {
            return Err(errors::encode(
                "invalid_params",
                "This operation needs a PR-mode review",
                json!({
                    "task_id":task_id,"recovery":"Use snapshot/act/complete for snapshot reviews."
                }),
            ));
        }
        Ok((project_id, task, review))
    }

    pub(crate) fn review_close(
        &mut self,
        params: ReviewCloseParams,
        actor: Actor,
    ) -> Result<Value, String> {
        params.validate()?;
        let (project_id, _, review) = self.pull_request(&params.task_id)?;
        version(&review, params.expected_version)?;
        if !review
            .pull_request
            .as_ref()
            .expect("PR checked")
            .status
            .is_active()
        {
            return Err(errors::encode(
                "conflict",
                "Only active unmerged PRs can close",
                json!({
                    "task_id":params.task_id,"recovery":"Reopen a Closed PR, or open a new PR after merge."
                }),
            ));
        }
        self.review_complete(crate::api::v1::reviews::ReviewCompleteParams {
            task_id:params.task_id.clone(),expected_version:params.expected_version,description:params.description,
        }, actor).map_err(|message| errors::service(message, json!({"task_id":params.task_id,"recovery":"Read tasks.review.get before retrying."})))?;
        invalidates(self, &project_id, &params.task_id, &review.workspace_id);
        let saved = self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .ok_or("unknown PR review")?;
        projection::result(self.tracker_store()?, saved)
    }

    pub(crate) fn review_reopen(
        &mut self,
        params: ReviewVersionParams,
        actor: Actor,
    ) -> Result<Value, String> {
        params.validate()?;
        let (project_id, _, review) = self.pull_request(&params.task_id)?;
        version(&review, params.expected_version)?;
        if review.pull_request.as_ref().expect("PR checked").status != PullRequestStatus::Closed {
            return Err(errors::encode(
                "conflict",
                "Only Closed unmerged PRs can reopen",
                json!({
                    "task_id":params.task_id,"recovery":"Open a new PR after merge."
                }),
            ));
        }
        let workspace = self.workspaces.get(&review.workspace_id).filter(|workspace| workspace.project_id == project_id)
            .cloned().ok_or_else(|| errors::encode("unavailable", "Original review workspace is unavailable", json!({
                "workspace_id":review.workspace_id,"recovery":"Restore the original managed workspace before reopening."
            })))?;
        self.refuse_writers_while_reserved(&workspace.root)?;
        let request = ReopenRequest {
            task_id: params.task_id,
            expected_version: params.expected_version,
            actor,
            workspace,
            sources: self
                .sources_for(&project_id)?
                .into_iter()
                .map(|source| crate::workspace::WorkspaceSource {
                    id: source.id,
                    name: source.name,
                    mount: source.mount,
                    path: source.path,
                    is_git: source.is_git,
                    base_branch: source.base_branch,
                })
                .collect(),
        };
        self.defer_review_sync(SyncJob {
            store: self.tracker_store()?.clone(),
            project_id,
            workspace_id: review.workspace_id,
            operation: SyncOperation::Reopen(request),
            hooks: WorkspaceHooks {
                state: self.self_handle.clone(),
            },
        });
        Ok(Value::Null)
    }

    pub(crate) fn review_refresh(
        &mut self,
        params: ReviewVersionParams,
        actor: Actor,
    ) -> Result<Value, String> {
        params.validate()?;
        let (project_id, _, review) = self.pull_request(&params.task_id)?;
        version(&review, params.expected_version)?;
        self.defer_review_sync(SyncJob {
            store: self.tracker_store()?.clone(),
            project_id,
            workspace_id: review.workspace_id,
            operation: SyncOperation::Refresh {
                task_id: params.task_id,
                expected_version: params.expected_version,
                actor,
            },
            hooks: WorkspaceHooks {
                state: self.self_handle.clone(),
            },
        });
        Ok(Value::Null)
    }

    fn defer_review_sync(&mut self, job: SyncJob) {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(job),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
    }
}
