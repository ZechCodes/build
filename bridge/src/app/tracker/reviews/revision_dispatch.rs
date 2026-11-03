//! The daemon settles revision hand-offs through the ordinary task dispatcher.
use crate::app::tracker::TaskWrite;
use crate::app::AppState;
use crate::tracker::TaskComment;
use crate::tracker::{Actor, Assignee};

impl AppState {
    pub(in crate::app) fn drain_review_revision_dispatches(&mut self) -> Result<(), String> {
        self.drain_review_revision_dispatches_at(crate::agent::now_ms() as i64)
    }

    pub(in crate::app) fn drain_review_revision_dispatches_at(
        &mut self,
        now_ms: i64,
    ) -> Result<(), String> {
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        for delivery in store
            .pending_review_revision_dispatches_at(now_ms)
            .map_err(|error| error.to_string())?
        {
            let result = self.deliver_review_revision(&delivery);
            let recipient = result
                .as_ref()
                .ok()
                .and_then(|recipient| recipient.clone())
                .or_else(|| {
                    result
                        .as_ref()
                        .err()
                        .and_then(|_| self.revision_attempted_recipient(&delivery.task_id))
                });
            let error = result.as_ref().err().map(String::as_str);
            store
                .settle_review_revision_dispatch_at(
                    &delivery.snapshot_id,
                    recipient.as_deref(),
                    error,
                    error.is_some_and(transient_revision_error),
                    now_ms,
                )
                .map_err(|error| error.to_string())?;
        }
        for notice in store
            .failed_review_revision_notices()
            .map_err(|error| error.to_string())?
        {
            self.post_revision_failure_notice(&notice)?;
            store
                .finish_review_revision_notice(&notice.snapshot_id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn revision_attempted_recipient(&mut self, task_id: &str) -> Option<String> {
        let (project, task) = self.tracker_task(task_id).ok()?;
        match task.assignee {
            Some(Assignee::Agent { agent_id }) => Some(agent_id),
            Some(Assignee::ProjectAgent) => self
                .project_conversation_run(&project)
                .and_then(|owner| self.project_pin(&owner).map(str::to_owned)),
            _ => None,
        }
    }

    fn post_revision_failure_notice(
        &mut self,
        notice: &crate::store::RevisionFailureNotice,
    ) -> Result<(), String> {
        let store = self.tracker_store()?;
        if store
            .load_tracker_comment(&notice.comment_id)
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return Ok(());
        }
        // A deleted task has no timeline left to notify; retire its notice too.
        if store
            .load_tracker_task(&notice.task_id)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
        let (project, mut task) = self.tracker_task(&notice.task_id)?;
        task.set_watched(true);
        task.dismissed_through = None;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(Actor::Build, task);
        write.comments.push(TaskComment {
            review_notice: Some(crate::tracker::ReviewNoticeKind::RevisionUndelivered),
            id: notice.comment_id.clone(),
            task_id: notice.task_id.clone(),
            author: Actor::Build,
            body: crate::templates::fill(
                crate::templates::MESSAGE_REVIEW_REVISION_UNDELIVERED,
                &[
                    ("number", &notice.number.to_string()),
                    ("reviewer", &notice.reviewer),
                    ("error", &notice.error),
                ],
            ),
            anchor: None,
            reply_to: None,
            opinion: None,
            mentions_user: true,
            notifies_user: false,
            refs: vec![],
            attachments: vec![],
            created_at: now.clone(),
            author_context: None,
        });
        self.commit_task_write(&project, write, &now)?;
        Ok(())
    }

    fn deliver_review_revision(
        &mut self,
        delivery: &crate::store::RevisionDispatch,
    ) -> Result<Option<String>, String> {
        let (project_id, task) = self.tracker_task(&delivery.task_id)?;
        let review = self
            .tracker_store()?
            .load_review_sync_state(&task.id)
            .map_err(|error| error.to_string())?
            .ok_or("review is unavailable")?;
        let metadata = review
            .pull_request
            .as_ref()
            .ok_or("review has no PR metadata")?;
        // A newer saved revision supersedes an undelivered older hand-off.
        if !metadata.status.is_active()
            || metadata.latest_published_snapshot_id.as_deref() != Some(&delivery.snapshot_id)
        {
            return Ok(None);
        }
        let operation_id = format!("review-revision-{}", delivery.snapshot_id);
        if let Some(operation) = self
            .tracker_store()?
            .operation(&operation_id)
            .map_err(|error| error.to_string())?
        {
            return Ok(Some(operation.delivery_target().1.to_owned()));
        }
        let Some(recipient) = self.revision_reviewer(&project_id, task.assignee.as_ref())? else {
            return Ok(None);
        };
        let baseline = self
            .tracker_store()?
            .review_revision_baseline(&task.id, &recipient, delivery.number)
            .map_err(|error| error.to_string())?;
        let note = revision_note(delivery, task.number, baseline.as_ref());
        self.dispatch_review_reviewer(
            &project_id,
            &task,
            &note,
            &operation_id,
            &Actor::Build,
            None,
        )?;
        Ok(Some(recipient))
    }

    fn revision_reviewer(
        &self,
        project_id: &str,
        assignee: Option<&Assignee>,
    ) -> Result<Option<String>, String> {
        match assignee {
            Some(Assignee::Agent { agent_id }) => {
                if self.entity_of_agent(agent_id).is_none() {
                    return Err("unavailable: review reviewer is unavailable".into());
                }
                self.agent_of_this_project(project_id, agent_id)?;
                Ok(Some(agent_id.clone()))
            }
            Some(Assignee::ProjectAgent) => self
                .project_conversation_run(project_id)
                .and_then(|owner| self.project_pin(&owner).map(str::to_owned))
                .map(Some)
                .ok_or_else(|| "unavailable: review coordinator is unavailable".into()),
            None | Some(Assignee::User) => Ok(None),
        }
    }
}

fn revision_note(
    delivery: &crate::store::RevisionDispatch,
    task_number: u64,
    baseline: Option<&(String, u64)>,
) -> String {
    match baseline {
        Some((snapshot_id, number)) => crate::templates::fill(
            crate::templates::MESSAGE_REVIEW_REVISION_READY,
            &[
                ("number", &delivery.number.to_string()),
                ("previous_number", &number.to_string()),
                ("previous_snapshot_id", snapshot_id),
                ("snapshot_id", &delivery.snapshot_id),
                ("task_number", &task_number.to_string()),
            ],
        ),
        None => crate::templates::fill(
            crate::templates::MESSAGE_REVIEW_REVISION_FULL,
            &[
                ("number", &delivery.number.to_string()),
                ("snapshot_id", &delivery.snapshot_id),
                ("task_number", &task_number.to_string()),
            ],
        ),
    }
}

fn transient_revision_error(error: &str) -> bool {
    error.starts_with("unavailable:")
        || error.starts_with("busy:")
        || error.contains("database is locked")
        || error.contains("usage limit")
}
