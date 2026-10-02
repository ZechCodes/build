//! Thin review adapters: resolve trusted task/workspace identities, then hand
//! Git to the shared service with the app lock released.

mod tools;

use super::{StoredAnswer, TaskWrite};
use crate::api::v1::reviews::{ReviewCompleteParams, ReviewDiffParams, ReviewSnapshotParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::actions::{ActionRequest, ActionSource, ReviewActParams};
use crate::reviews::records::Review;
use crate::reviews::service::SnapshotRequest;
use crate::store::Store;
use crate::tracker::Actor;
use serde_json::{json, Value};

struct SnapshotJob {
    store: Store,
    project_id: String,
    request: SnapshotRequest,
}

struct ActionJob {
    store: Store,
    project_id: String,
    task_id: String,
    request: ActionRequest,
    changes: std::sync::Arc<crate::changes::ChangeBus>,
}

impl DeferredGitWork for ActionJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let review = crate::reviews::actions::act(&self.store, &self.request, || {
            self.changes
                .note_tasks(&self.project_id, std::slice::from_ref(&self.task_id));
        })?;
        Ok(json!({ "review": review }))
    }

    fn invalidate(&self, app: &mut AppState) {
        app.note_tasks_changed(&self.project_id, &self.task_id);
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
}

impl DeferredGitWork for SnapshotJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let review = crate::reviews::service::snapshot(&self.store, &self.request)?;
        Ok(json!({ "review": review }))
    }

    fn invalidate(&self, app: &mut AppState) {
        app.note_tasks_changed(&self.project_id, &self.request.task_id);
    }
}

impl AppState {
    pub(crate) fn review_snapshot(
        &mut self,
        params: ReviewSnapshotParams,
        author: Actor,
    ) -> Result<Value, String> {
        let (project_id, _) = self.tracker_task(&params.task_id)?;
        let workspace = self
            .workspaces
            .get(&params.workspace_id)
            .filter(|workspace| workspace.project_id == project_id)
            .ok_or_else(|| format!("unknown workspace_id: {}", params.workspace_id))?
            .clone();
        self.refuse_writers_while_reserved(&workspace.root)?;
        for directory in &workspace.directories {
            self.refuse_writers_while_reserved(&directory.path)?;
        }
        let job = SnapshotJob {
            store: self.tracker_store()?.clone(),
            project_id,
            request: SnapshotRequest {
                task_id: params.task_id,
                workspace,
                expected_version: params.expected_version,
                base_overrides: params.base_overrides,
                author,
            },
        };
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(job),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(Value::Null)
    }

    pub(crate) fn review_get(&mut self, task_id: &str) -> Result<Value, String> {
        let (project_id, _) = self.tracker_task(task_id)?;
        let review = self.tracker_store()?.load_review(task_id).stored()?;
        let Some(review) = review else {
            return Ok(json!({ "review": null }));
        };
        let sources = self.review_action_sources(&project_id, &review);
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            Ok(json!({ "review": crate::reviews::actions::with_destinations(review, &sources) }))
        })));
        Ok(Value::Null)
    }

    pub(crate) fn review_act(
        &mut self,
        params: ReviewActParams,
        actor: Actor,
    ) -> Result<Value, String> {
        let (project_id, _) = self.tracker_task(&params.task_id)?;
        let review = self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .ok_or_else(|| format!("unknown review for task_id: {}", params.task_id))?;
        let sources = self.review_action_sources(&project_id, &review);
        let task_id = params.task_id.clone();
        let job = ActionJob {
            store: self.tracker_store()?.clone(),
            project_id,
            task_id,
            request: ActionRequest {
                params,
                actor,
                sources,
            },
            changes: self.changes.clone(),
        };
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(job),
            params: Value::Null,
            invalidates: true,
            #[cfg(test)]
            gate: None,
        })));
        Ok(Value::Null)
    }

    fn review_action_sources(&self, project_id: &str, review: &Review) -> Vec<ActionSource> {
        let configured = self.sources_for(project_id).unwrap_or_default();
        review
            .snapshots
            .iter()
            .flat_map(|snapshot| snapshot.directories.iter())
            .map(|directory| {
                let source = configured
                    .iter()
                    .find(|source| source.id == directory.source_id);
                let source_path = source.map_or_else(
                    || directory.source_path.clone(),
                    |source| source.path.clone(),
                );
                let error = if source.is_none() {
                    Some(format!(
                        "source {} is no longer configured",
                        directory.source_id
                    ))
                } else {
                    self.review_source_reservation(project_id, directory, &source_path)
                        .err()
                };
                ActionSource {
                    directory: directory.clone(),
                    source_path,
                    error,
                }
            })
            .collect()
    }

    fn review_source_reservation(
        &self,
        project_id: &str,
        directory: &crate::reviews::model::ReviewDirectory,
        source_path: &std::path::Path,
    ) -> Result<(), String> {
        self.refuse_writers_while_reserved(&directory.path)?;
        self.refuse_writers_while_reserved(source_path)?;
        for workspace in self.workspaces.list(Some(project_id)) {
            for checkout in &workspace.directories {
                if checkout.source_id == directory.source_id {
                    self.refuse_writers_while_reserved(&checkout.path)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn review_diff(&mut self, params: ReviewDiffParams) -> Result<Value, String> {
        self.tracker_task(&params.task_id)?;
        let review = self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .ok_or_else(|| format!("unknown review for task_id: {}", params.task_id))?;
        let snapshot = review
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == params.snapshot_id)
            .ok_or_else(|| format!("unknown snapshot_id: {}", params.snapshot_id))?;
        let directory = snapshot
            .directories
            .iter()
            .find(|directory| directory.id == params.directory_id)
            .ok_or_else(|| format!("unknown directory_id: {}", params.directory_id))?
            .clone();
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            let read = crate::reviews::read::read(&directory, &params.read)?;
            serde_json::to_value(read).map_err(|error| error.to_string())
        })));
        Ok(Value::Null)
    }

    pub(crate) fn review_complete(
        &mut self,
        params: ReviewCompleteParams,
        actor: Actor,
    ) -> Result<Value, String> {
        let (project_id, _) = self.tracker_task(&params.task_id)?;
        let (review, events) = self
            .tracker_store()?
            .complete_review_with_events(
                &params.task_id,
                params.expected_version,
                &actor,
                &params.description,
            )
            .stored()?;
        let (_, task) = self.tracker_task(&params.task_id)?;
        let mut write = TaskWrite::by(actor, task);
        write.events = events;
        self.publish_task_write(&project_id, &write);
        Ok(json!({ "review": review }))
    }
}
