//! Caller-owned board inputs prepared in the transaction changing the graph.

use super::*;
use crate::store::{ReadTransaction, TrackerTaskPublication};

impl Store {
    pub(crate) fn create_tracker_task_waiting_on_observed(
        &self,
        draft: Task,
        events: &[TaskEvent],
        waiting_on: &[String],
    ) -> Result<TrackerTaskPublication, StoreError> {
        self.in_waiting_on_transaction(|tx| {
            let task = create_task_in_tx(tx, draft, events, waiting_on)?;
            ReadTransaction::new(tx).tracker_task_publication(task, &[])
        })
    }

    pub(crate) fn edit_waiting_on_observed(
        &self,
        project: &str,
        task_id: &str,
        add: &[String],
        remove: &[String],
        actor: &Actor,
        now: &str,
    ) -> Result<TrackerTaskPublication, StoreError> {
        self.in_waiting_on_transaction(|tx| {
            let task = require_task(tx, project, task_id)?;
            graph::edit(tx, &task, add, remove, actor, now)?;
            let task = require_task(tx, project, task_id)?;
            ReadTransaction::new(tx).tracker_task_publication(task, remove)
        })
    }

    pub(crate) fn save_tracker_task_waiting_on_observed(
        &self,
        task: &Task,
        comments: &[TaskComment],
        events: &[TaskEvent],
        add: &[String],
        remove: &[String],
        actor: &Actor,
    ) -> Result<(TrackerTaskPublication, Vec<TaskEvent>), StoreError> {
        self.in_waiting_on_transaction(|tx| {
            let (task, additional) =
                save_activity_and_dependencies(tx, task, comments, events, add, remove, actor)?;
            let publication = ReadTransaction::new(tx).tracker_task_publication(task, remove)?;
            Ok((publication, additional))
        })
    }
}
