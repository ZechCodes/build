//! Durable completion keeps its existing task side effects; observation is deferred.
use super::{projection, version};
use crate::api::v1::reviews::{errors, ReviewCloseParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::tracker::reviews::observation::ReviewInvalidation;
use crate::app::tracker::StoredAnswer;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use serde_json::{json, Value};

struct CloseJob {
    store: Store,
    project: String,
    task: String,
    review: Result<Review, String>,
    observation: ReviewInvalidation,
}

impl DeferredGitWork for CloseJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let result = self
            .review
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|review| projection::result(&self.store, review.clone()));
        self.observation.prepare();
        result
    }

    fn invalidate(&self, app: &mut AppState) {
        self.observation.invalidate(app, &self.project, &self.task);
    }

    fn invalidates_on_error(&self) -> bool {
        true
    }
}

impl AppState {
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
        let saved = self
            .tracker_store()?
            .load_review(&params.task_id)
            .stored()
            .and_then(|review| review.ok_or_else(|| "unknown PR review".to_owned()));
        let job = CloseJob {
            store: self.tracker_store()?.clone(),
            project: project_id,
            task: params.task_id,
            observation: ReviewInvalidation::new(self, &review.workspace_id),
            review: saved,
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
}
