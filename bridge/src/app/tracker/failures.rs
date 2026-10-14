//! A failed harness cannot write its own task handoff. Keep the same task
//! comment and unread channel, without waking that failed holder to read it.

use super::{StoredAnswer, TaskWrite};
use crate::app::AppState;
use crate::tracker::{Actor, Task, TaskComment};

impl AppState {
    pub(in crate::app) fn comment_terminal_agent_failure(
        &mut self,
        mut task: Task,
        failure_key: &str,
        body: &str,
    ) -> Result<(), String> {
        let project = self.tracker_task(&task.id)?.0;
        let id = format!("tc-agent-failure-{failure_key}");
        if self
            .tracker_store()?
            .load_tracker_comment(&id)
            .stored()?
            .is_some()
        {
            return Ok(());
        }
        let now = crate::store::now_rfc3339();
        task.updated_at = now.clone();
        let mut write = TaskWrite::by(Actor::Build, task);
        write.comments.push(TaskComment {
            id,
            task_id: write.task.id.clone(),
            author: Actor::Build,
            body: body.into(),
            refs: Vec::new(),
            attachments: Vec::new(),
            created_at: now.clone(),
            author_context: None,
            anchor: None,
            reply_to: None,
            opinion: None,
            mentions_user: false,
            notifies_user: true,
        });
        let (saved, _) = self
            .tracker_store()?
            .save_review_task_activity(&write.task, &write.comments, &[], &Actor::Build, None, &now)
            .stored()?;
        write.task = saved;
        self.note_tasks_changed(&project, &write.task.id);
        self.push_task_news(&write);
        Ok(())
    }
}
