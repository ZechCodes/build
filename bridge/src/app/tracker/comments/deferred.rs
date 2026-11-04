//! Commit anchors resolve immutable Git inputs under the app lock, validate
//! off-lock, then write against the task and review metadata that exist now.
use super::{AnchorCommitCheck, CommentMetadata};
use crate::app::git::deferred::DeferredGitWork;
use crate::app::tracker::tools::DeferredCommentPost;
use crate::app::tracker::{refs, TaskWrite};
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::tracker::{Actor, TaskComment, TaskEventKind};
use serde_json::{json, Value};

pub(in crate::app::tracker) struct DeferredCommentWrite {
    pub project_id: String,
    pub comment: TaskComment,
    /// Captured before Git work; a later revision must refuse an old opinion.
    pub expected_review_version: Option<u64>,
    pub check: AnchorCommitCheck,
    pub post: Option<DeferredCommentPost>,
}

impl DeferredGitWork for DeferredCommentWrite {
    fn run(&self, _: &Value) -> Result<Value, String> {
        crate::reviews::read::validate_commit(&self.check.directory, &self.check.commit)?;
        Ok(Value::Null)
    }

    fn invalidate(&self, _: &mut AppState) {}

    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        if let Some(post) = &self.post {
            app.validate_deferred_comment_caller(post, &self.comment.task_id)?;
        }
        let (project_id, task) = app.tracker_task(&self.comment.task_id)?;
        if project_id != self.project_id {
            return Err(format!("unknown task_id: {}", self.comment.task_id));
        }
        self.revalidate_metadata(app)?;
        refs::fenced_refs(
            &json!({"refs": self.comment.refs}),
            &task,
            &app.task_checkout_ids(&task),
        )?;
        let now = crate::store::now_rfc3339();
        let mut comment = self.comment.clone();
        comment.created_at = now.clone();
        let mut write = TaskWrite::by(comment.author.clone(), task);
        write.expected_review_version = self.expected_review_version;
        write.comments.push(comment.clone());
        if comment.author == Actor::User && write.task.set_watched(true) {
            write.event(&Actor::User, TaskEventKind::Watched, json!({}), &now);
        }
        let saved = app.commit_task_write(&project_id, write, &now)?;
        let answer = json!({"task":saved["task"], "comment":comment});
        match &self.post {
            Some(post) => app.finish_deferred_comment_post(post, answer),
            None => Ok(answer),
        }
    }
}

impl DeferredCommentWrite {
    fn revalidate_metadata(&self, app: &AppState) -> Result<(), String> {
        let metadata = CommentMetadata::from_params(
            app,
            &self.comment.task_id,
            &json!({
                "anchor":self.comment.anchor,
                "reply_to":self.comment.reply_to,
                "opinion":self.comment.opinion,
            }),
        )?;
        // Snapshot directories are immutable. Check identity again so removal
        // or replacement during validation cannot authorize another range.
        if metadata.commit_check.as_ref() != Some(&self.check) {
            return Err(crate::api::v1::reviews::errors::encode(
                "conflict",
                "The anchored revision changed during validation",
                json!({"task_id":self.comment.task_id}),
            ));
        }
        Ok(())
    }
}

impl AppState {
    pub(in crate::app::tracker) fn defer_task_comment(
        &mut self,
        write: DeferredCommentWrite,
    ) -> Result<Value, String> {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(write),
            params: Value::Null,
            invalidates: false,
            registry_change: false,
            #[cfg(test)]
            gate: None,
        })));
        Ok(Value::Null)
    }
}
