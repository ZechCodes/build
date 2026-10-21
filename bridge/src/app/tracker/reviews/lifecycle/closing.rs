//! Complete off lock, then publish the captured task and workspace together.
use super::{projection, version};
use crate::api::v1::reviews::{errors, ReviewCloseParams};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::tracker::reviews::observation::ReviewInvalidation;
use crate::app::tracker::{StoredAnswer, TaskWrite};
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Actor;
use serde_json::{json, Value};
use std::cell::RefCell;

struct CloseJob {
    store: Store,
    project: String,
    params: ReviewCloseParams,
    actor: Actor,
    completion: RefCell<Option<TaskWrite>>,
    observation: ReviewInvalidation,
}

impl CloseJob {
    fn complete(&self) -> Result<Review, String> {
        let (review, task, events) = self.store.complete_review_with_task(
            &self.params.task_id, self.params.expected_version, &self.actor, &self.params.description,
        ).stored().map_err(|message| errors::service(message, json!({
            "task_id":self.params.task_id,"recovery":"Read tasks.review.get before retrying."
        })))?;
        let mut write = TaskWrite::by(self.actor.clone(), task);
        write.events = events;
        *self.completion.borrow_mut() = Some(write);
        Ok(review)
    }

    fn publish_completion(&self, app: &mut AppState) {
        self.observation.install(app);
        if let Some(write) = self.completion.take() {
            app.publish_task_write(&self.project, &write);
        }
    }
}

impl DeferredGitWork for CloseJob {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let result = self.complete().map(|review| {
            projection::result(&self.store, review.clone()).unwrap_or_else(|error| {
                // Completion is durable; optional observations cannot refuse it.
                eprintln!("closed review projection {}: {error}", review.task_id);
                json!({"review": review})
            })
        });
        self.observation.prepare();
        result
    }

    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        self.publish_completion(app);
        Ok(result)
    }

    fn invalidate(&self, app: &mut AppState) {
        // Settlement and invalidation share the captured write so completion
        // notices publish once even when both paths run.
        self.publish_completion(app);
        super::invalidates(app, self.observation.workspace());
    }

    fn invalidates_on_error(&self) -> bool {
        self.completion.borrow().is_some()
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
                json!({"task_id":params.task_id,"recovery":"Reopen a Closed PR, or open a new PR after merge."}),
            ));
        }
        let job = CloseJob {
            store: self.tracker_store()?.clone(),
            project: project_id,
            params,
            actor,
            completion: RefCell::default(),
            observation: ReviewInvalidation::new(self, &review.workspace_id),
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
