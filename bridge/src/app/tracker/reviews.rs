//! Thin review adapters: resolve trusted task/workspace identities, then hand
//! Git to the shared service with the app lock released.

mod tools;

use super::{StoredAnswer, TaskWrite};
use crate::api::v1::reviews::{ReviewCompleteParams, ReviewDiffParams, ReviewSnapshotParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::service::SnapshotRequest;
use crate::store::Store;
use crate::tracker::{Actor, TaskEventKind, TimelineEntry};
use serde_json::{json, Value};

struct SnapshotJob {
    store: Store,
    project_id: String,
    request: SnapshotRequest,
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
        self.tracker_task(task_id)?;
        let review = self.tracker_store()?.load_review(task_id).stored()?;
        Ok(json!({ "review": review }))
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
        let review = self
            .tracker_store()?
            .complete_review(
                &params.task_id,
                params.expected_version,
                &actor,
                &params.description,
            )
            .stored()?;
        self.note_tasks_changed(&project_id, &params.task_id);
        self.nudge_workspace_reclaim();
        self.notify_review_completion(&params.task_id, actor);
        Ok(json!({ "review": review }))
    }

    /// Notification failure cannot undo a durable completion. The event and
    /// task were written together by the service, before any tracker is woken.
    fn notify_review_completion(&mut self, task_id: &str, actor: Actor) {
        let Ok((_, task)) = self.tracker_task(task_id) else {
            return;
        };
        let Ok(timeline) = self
            .tracker_store()
            .and_then(|store| store.load_tracker_timeline(task_id).stored())
        else {
            return;
        };
        let mut write = TaskWrite::by(actor, task);
        if let Some(event) = timeline.into_iter().rev().find_map(|entry| match entry {
            TimelineEntry::Event(event) if event.kind == TaskEventKind::ReviewCompleted => {
                Some(event)
            }
            _ => None,
        }) {
            write.events.push(event);
            self.notify_trackers(&write);
            self.push_task_news(&write);
        }
    }
}
