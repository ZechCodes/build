//! The daemon settles revision hand-offs through the ordinary task dispatcher.
use crate::app::AppState;
use crate::tracker::Actor;

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
                    result.as_ref().err().map(String::as_str),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn deliver_review_revision(
        &mut self,
        delivery: &crate::store::RevisionDispatch,
    ) -> Result<(), String> {
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
            return Ok(());
        }
        let note = crate::templates::fill(
            crate::templates::MESSAGE_REVIEW_REVISION_READY,
            &[
                ("number", &delivery.number.to_string()),
                ("previous_number", &(delivery.number - 1).to_string()),
                ("previous_snapshot_id", &delivery.previous_snapshot_id),
                ("snapshot_id", &delivery.snapshot_id),
                ("task_number", &task.number.to_string()),
            ],
        );
        self.dispatch_review_reviewer(
            &project_id,
            &task,
            &note,
            &format!("review-revision-{}", delivery.snapshot_id),
            &Actor::Build,
            None,
        )
    }
}
