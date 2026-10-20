//! Owned tracker projection inputs, captured before a write can commit.

use super::{
    DeliveryIntent, DependencySummary, DependentsPage, ReadTransaction, Store, StoreError,
    TaskFilter,
};
use crate::reviews::model::ReviewSummary;
use crate::tracker::{Task, TimelineEntry};
use std::collections::BTreeSet;

pub(crate) struct TrackerTaskPublication {
    pub current: TrackerTaskObservation,
    pub related: Vec<TrackerTaskObservation>,
}

pub(crate) struct TrackerTaskObservation {
    pub task: Task,
    pub timeline: Vec<TimelineEntry>,
    pub review: Option<ReviewSummary>,
    pub dependencies: DependencySummary,
    pub delivery: Option<DeliveryIntent>,
    pub blocks: DependentsPage,
}

impl ReadTransaction<'_> {
    pub(crate) fn tracker_task_publication(
        &self,
        task: Task,
        removed_dependencies: &[String],
    ) -> Result<TrackerTaskPublication, StoreError> {
        let mut related_ids = self.tracker_task_neighbors(&task.id)?;
        related_ids.extend(removed_dependencies.iter().cloned());
        related_ids.remove(&task.id);
        let mut related = Vec::new();
        for id in related_ids {
            let Some(neighbor) = self.load_tracker_task(&id)? else {
                continue;
            };
            if neighbor.project_path != task.project_path {
                return Err(StoreError::WaitingOnInvalid(
                    "dependency crosses task project".into(),
                ));
            }
            if neighbor.watched {
                related.push(self.tracker_task_observation(neighbor)?);
            }
        }
        Ok(TrackerTaskPublication {
            current: self.tracker_task_observation(task)?,
            related,
        })
    }

    fn tracker_task_neighbors(&self, task: &str) -> Result<BTreeSet<String>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id = ?1
             UNION SELECT waiting_on_task_id FROM tracker_task_dependencies WHERE task_id = ?1",
        )?;
        let rows = statement.query_map([task], |row| row.get(0))?;
        rows.collect::<Result<_, _>>().map_err(StoreError::from)
    }

    pub(crate) fn tracker_task_observation(
        &self,
        task: Task,
    ) -> Result<TrackerTaskObservation, StoreError> {
        Ok(TrackerTaskObservation {
            timeline: self.load_tracker_timeline(&task.id)?,
            review: self.load_review_summary(&task.id)?,
            dependencies: self.waiting_on_summary(&task.project_path, &task.id)?,
            delivery: self.current_delivery_intent(&task.id)?,
            blocks: self.waiting_on_blocks(&task.project_path, &task.id, 20, None)?,
            task,
        })
    }
}

impl Store {
    /// A project observation is all-or-nothing. A broken timeline or related
    /// summary cannot masquerade as a deliberately removed watched task.
    pub(crate) fn watched_tracker_task_observations(
        &self,
        project: &str,
    ) -> Result<Vec<TrackerTaskObservation>, StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let reader = ReadTransaction::new(&transaction);
        let observations = reader
            .list_tracker_tasks(project, TaskFilter::default())?
            .into_iter()
            .filter(|task| task.watched)
            .map(|task| reader.tracker_task_observation(task))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit()?;
        Ok(observations)
    }
}
