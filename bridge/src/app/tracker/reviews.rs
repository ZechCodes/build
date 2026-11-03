//! Thin review adapters: resolve trusted task/workspace identities, then hand
//! Git to the shared service with the app lock released.

mod lifecycle;
mod merge;
mod observation;
mod opening;
mod projection;
mod publication;
mod revision_dispatch;
mod tools;
mod verdict_delivery;

use super::{StoredAnswer, TaskWrite};
use crate::api::v1::reviews::{ReviewCompleteParams, ReviewDiffParams, ReviewSnapshotParams};
use crate::app::{AppState, DeferredWork};
use crate::reviews::actions::{ActionSource, ReviewActParams};
use crate::reviews::records::Review;
use crate::tracker::Actor;
use serde_json::{json, Value};

impl AppState {
    pub(crate) fn review_snapshot(
        &mut self,
        params: ReviewSnapshotParams,
        author: Actor,
    ) -> Result<Value, String> {
        self.tracker_task(&params.task_id)?;
        if self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .is_some_and(|review| review.mode == crate::reviews::model::ReviewMode::PullRequest)
        {
            let (_, _, review) = self.pull_request(&params.task_id)?;
            if params.workspace_id != review.workspace_id || !params.base_overrides.is_empty() {
                return Err(crate::api::v1::reviews::errors::encode(
                    "invalid_params",
                    "PR snapshots only reconcile the bound receiving refs",
                    json!({
                        "task_id":params.task_id,"workspace_id":review.workspace_id,
                        "recovery":"Use tasks.review.update to retarget bases; open a new PR for another workspace."
                    }),
                ));
            }
            return self.review_refresh(
                crate::api::v1::reviews::ReviewVersionParams {
                    task_id: params.task_id,
                    expected_version: params.expected_version,
                },
                author,
            );
        }
        Err(crate::api::v1::reviews::errors::snapshot_retired(
            &params.task_id,
        ))
    }

    pub(crate) fn review_get(&mut self, task_id: &str) -> Result<Value, String> {
        let (project_id, _) = self.tracker_task(task_id)?;
        let review = self.tracker_store()?.load_review(task_id).stored()?;
        let Some(review) = review else {
            return Ok(json!({ "review": null }));
        };
        let sources = self.review_action_sources(&project_id, &review);
        let store = self.tracker_store()?.clone();
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            projection::result(
                &store,
                crate::reviews::actions::with_destinations(review, &sources),
            )
        })));
        Ok(Value::Null)
    }

    pub(crate) fn review_act(
        &mut self,
        params: ReviewActParams,
        _actor: Actor,
    ) -> Result<Value, String> {
        self.tracker_task(&params.task_id)?;
        let review = self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .ok_or_else(|| format!("unknown review for task_id: {}", params.task_id))?;
        if review.mode == crate::reviews::model::ReviewMode::Snapshot {
            return Err(crate::api::v1::reviews::errors::snapshot_retired(
                &params.task_id,
            ));
        }
        Err(crate::api::v1::reviews::errors::encode(
            "invalid_params",
            "PR actions must use the PR merge lifecycle service",
            json!({"task_id":params.task_id,"replacement":"tasks.review.merge"}),
        ))
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
        let earlier = crate::api::v1::reviews::earlier_directory(&review, &params)?;
        self.deferred_work = Some(DeferredWork::External(Box::new(move || {
            let read = match earlier.as_ref() {
                Some(earlier) => {
                    crate::reviews::read::read_since(&directory, earlier, &params.read)?
                }
                None => crate::reviews::read::read(&directory, &params.read)?,
            };
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
        if self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()?
            .is_some_and(|review| review.mode == crate::reviews::model::ReviewMode::PullRequest)
        {
            return Err(crate::api::v1::reviews::errors::encode(
                "invalid_params",
                "Close a pull request with tasks.review.close",
                json!({"task_id":params.task_id,"replacement":"tasks.review.close"}),
            ));
        }
        let (review, events) = self
            .tracker_store()?
            .finish_review_with_events(
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
