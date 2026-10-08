//! Resolve saved merge destinations; explicit retries resume one durable plan.
use super::lifecycle::{invalidates, version};
use super::super::StoredAnswer;
use crate::api::v1::reviews::{errors, ReviewMergeParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::merge::MergeJob;
use crate::reviews::model::{ReviewMergePush, ReviewMergeRequest, ReviewMergeSource};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

struct MergeWork {
    store: Store,
    project_id: String,
    workspace_id: String,
    job: MergeJob,
    changes: Arc<crate::changes::ChangeBus>,
}

impl DeferredGitWork for MergeWork {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let review = crate::reviews::merge::merge(&self.store, &self.job, || {
            self.changes.note_tasks(&self.project_id, std::slice::from_ref(&self.job.request.task_id));
            self.changes.note_kind(&self.workspace_id, crate::changes::Kind::State);
            self.changes.note_board_lists(crate::changes::BoardLists::WORKSPACES);
        }).map_err(|message| errors::service(message, json!({
            "task_id":self.job.request.task_id,"snapshot_id":self.job.request.snapshot_id,
            "recovery":"Read tasks.review.get and retry the saved merge plan after resolving its reported failure."
        })))?;
        super::projection::result(&self.store, review)
    }

    fn invalidate(&self, app: &mut AppState) {
        invalidates(app, &self.project_id, &self.job.request.task_id, &self.workspace_id);
    }

    fn invalidates_on_error(&self) -> bool { true }
}

impl AppState {
    pub(crate) fn review_merge(&mut self, mut params: ReviewMergeParams, actor: Actor) -> Result<Value, String> {
        params.validate()?;
        params.sources.sort_by(|left,right| left.directory_id.cmp(&right.directory_id));
        let (project_id, task, review) = self.pull_request(&params.task_id)?;
        let request_id = merge_identity(&params, &actor)?;
        let store = self.tracker_store()?.clone();
        let existing = store.load_review_merge_intent(&task.project_path, &request_id).stored()?;
        let request = if let Some(intent) = existing {
            if params.expected_version != intent.request.expected_version {
                version(&review, params.expected_version)?;
            }
            intent.request
        } else {
            version(&review, params.expected_version)?;
            if !review.pull_request.as_ref().expect("PR checked").status.is_active() {
                return Err(errors::encode("conflict", "Merge requires an active unmerged PR", json!({
                    "task_id":params.task_id,"recovery":"Retry a retained merge plan, or open a new PR after merge."
                })));
            }
            merge_request(&review, &params, actor)?
        };
        let sources = self.review_action_sources(&project_id, &review);
        let work = MergeWork { store,project_id,workspace_id:review.workspace_id.clone(),
            job:MergeJob {project_path:task.project_path,request_id,request,sources},changes:self.changes.clone() };
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call:Box::new(work),params:Value::Null,invalidates:true,
            #[cfg(test)] gate:None,
        })));
        Ok(Value::Null)
    }
}

fn merge_identity(params: &ReviewMergeParams, actor: &Actor) -> Result<String, String> {
    let encoded = serde_json::to_vec(&(&params.task_id, &params.snapshot_id, actor, &params.sources))
        .map_err(|error| error.to_string())?;
    Ok(format!("pr-merge-{:x}", Sha256::digest(encoded)))
}

fn merge_request(review: &Review, params: &ReviewMergeParams, actor: Actor) -> Result<ReviewMergeRequest, String> {
    let snapshot = review.snapshots.iter().find(|snapshot| snapshot.id == params.snapshot_id)
        .ok_or_else(|| format!("unknown snapshot_id: {}", params.snapshot_id))?;
    if review.pull_request.as_ref().and_then(|metadata| metadata.latest_published_snapshot_id.as_deref()) != Some(&params.snapshot_id) {
        return Err(errors::encode("stale_version", "Merge needs the current published snapshot", json!({
            "task_id":params.task_id,"recovery":"Read tasks.review.get and select its latest snapshot."
        })));
    }
    if params.sources.len() != review.bindings.len() {
        return Err(errors::encode("invalid_params", "Merge must select every included Git directory", json!({
            "task_id":params.task_id,"recovery":"Select all bindings from tasks.review.get."
        })));
    }
    let sources = params.sources.iter().map(|source| {
        let binding = review.bindings.iter().find(|binding| binding.directory_id == source.directory_id)
            .ok_or_else(|| format!("unknown directory_id: {}", source.directory_id))?;
        let directory = snapshot.directories.iter().find(|directory| directory.id == source.directory_id)
            .ok_or_else(|| format!("unknown directory_id: {}", source.directory_id))?;
        Ok(ReviewMergeSource {
            directory_id:source.directory_id.clone(),repository_id:binding.repository_id.clone(),
            base_branch_ref:binding.base_branch_ref.clone(),head:directory.head.clone().ok_or("PR directory has no saved head")?,
            expected_base_head:source.expected_base_head.clone(),push:source.push.as_ref().map(|push| ReviewMergePush {
                remote:push.remote.clone(),branch:push.branch.clone(),
            }),
        })
    }).collect::<Result<Vec<_>, String>>()?;
    Ok(ReviewMergeRequest {task_id:params.task_id.clone(),expected_version:params.expected_version,
        snapshot_id:params.snapshot_id.clone(),actor,sources})
}
