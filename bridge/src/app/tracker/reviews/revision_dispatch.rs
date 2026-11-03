//! The daemon settles revision hand-offs through the ordinary task dispatcher.
use crate::app::AppState;
use crate::tracker::{Actor, Assignee};

impl AppState {
    pub(in crate::app) fn drain_review_revision_dispatches(&mut self) -> Result<(), String> {
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        for delivery in store
            .pending_review_revision_dispatches()
            .map_err(|error| error.to_string())?
        {
            let result = self.deliver_review_revision(&delivery);
            store
                .settle_review_revision_dispatch(
                    &delivery.snapshot_id,
                    result
                        .as_ref()
                        .ok()
                        .and_then(|recipient| recipient.as_deref()),
                    result.as_ref().err().map(String::as_str),
                )
                .map_err(|error| error.to_string())?;
        }
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
            Some(Assignee::Agent { agent_id }) => Ok(Some(agent_id.clone())),
            Some(Assignee::ProjectAgent) => self
                .project_conversation_run(project_id)
                .and_then(|owner| self.project_pin(&owner).map(str::to_owned))
                .map(Some)
                .ok_or_else(|| "review coordinator is unavailable".into()),
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
