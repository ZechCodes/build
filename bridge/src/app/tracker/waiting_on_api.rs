//! Narrow transport adapters over the durable dependency and delivery services.
use super::{StoredAnswer, TaskWrite};
use crate::app::{require_str, AppState};
use crate::tracker::Actor;
use serde_json::{json, Value};

pub(super) fn waiting_ids(params: &Value) -> Result<Vec<String>, String> {
    let Some(values) = params.get("waiting_on") else {
        return Ok(Vec::new());
    };
    let values = values.as_array().ok_or("waiting_on must be an array")?;
    if values.len() > 100 {
        return Err("waiting_on exceeds 100 input ids".into());
    }
    let mut ids = values
        .iter()
        .map(|id| {
            let id = id
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or("waiting_on must contain nonempty task ids")?;
            if id.len() > 128 {
                return Err("waiting_on task id exceeds 128 bytes".into());
            }
            Ok(id.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    if ids.len() > 20 {
        return Err("waiting_on exceeds 20 distinct tasks".into());
    }
    Ok(ids)
}

impl AppState {
    pub(crate) fn tasks_waiting_on_edit(
        &mut self,
        params: &Value,
        add: bool,
    ) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let (project, task) = self.tracker_task(&task_id)?;
        let ids = waiting_ids(params)?;
        self.validate_task_waiting_ids(&task.project_path, &ids)?;
        if ids.is_empty() {
            return Err("waiting_on needs one to 20 ids".into());
        }
        self.edit_task_waiting_on(&project, &task, &ids, add, &Actor::User)
    }

    pub(super) fn validate_task_waiting_ids(
        &mut self,
        path: &str,
        ids: &[String],
    ) -> Result<(), String> {
        for id in ids {
            let (_, task) = self.tracker_task(id)?;
            if task.project_path != path {
                return Err(format!("unknown task_id: {id}"));
            }
        }
        Ok(())
    }

    pub(super) fn edit_task_waiting_on(
        &mut self,
        project: &str,
        task: &crate::tracker::Task,
        ids: &[String],
        add: bool,
        actor: &Actor,
    ) -> Result<Value, String> {
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?;
        let mut prepared = task.clone();
        self.capture_task_identities_and_actor(&mut prepared, &timeline, actor);
        #[cfg(test)]
        let retiring = self.task_delivery_publication_candidate(&task.id, false)?;
        let publication = self
            .tracker_store()?
            .edit_waiting_on_observed(
                &prepared,
                if add { ids } else { &[] },
                if add { &[] } else { ids },
                actor,
                &crate::store::now_rfc3339(),
            )
            .stored()?;
        self.note_observed_tasks_changed_with_retirement(
            project,
            &publication,
            #[cfg(test)]
            retiring.as_ref().filter(|_| {
                publication
                    .current
                    .delivery
                    .as_ref()
                    .is_some_and(|intent| intent.state == "waiting")
            }),
        );
        Ok(json!({"task":self.observed_task_json(project, &publication.current)}))
    }

    pub(super) fn task_dependency_answer(&mut self, id: &str) -> Result<Value, String> {
        let read = self.tasks_get(&json!({"task_id":id}))?;
        Ok(json!({"task":read["task"]}))
    }

    pub(crate) fn tasks_waiting_on_blocks(&self, params: &Value) -> Result<Value, String> {
        self.task_read_context()
            .tasks_waiting_on_blocks(self.tracker_store()?, params)
    }

    pub(super) fn commit_task_dependency_write(
        &mut self,
        project: &str,
        mut write: TaskWrite,
        ids: &[String],
        now: &str,
    ) -> Result<Value, String> {
        write.task.updated_at = now.to_string();
        let mut timeline = self
            .tracker_store()?
            .load_tracker_timeline(&write.task.id)
            .stored()?;
        timeline.extend(
            write
                .events
                .iter()
                .cloned()
                .map(crate::tracker::TimelineEntry::Event),
        );
        self.capture_task_identities_and_actor(&mut write.task, &timeline, &write.actor);
        let (publication, additional) = self
            .tracker_store()?
            .save_tracker_task_waiting_on_observed(
                &write.task,
                &write.comments,
                &write.events,
                ids,
                &[],
                &write.actor,
            )
            .stored()?;
        write.task = publication.current.task.clone();
        write.events.extend(additional);
        self.publish_observed_task_write(project, &write, &publication);
        Ok(json!({"task":self.observed_task_json(project, &publication.current)}))
    }

    pub(super) fn retry_task_delivery_as(
        &mut self,
        task_id: &str,
        id: &str,
    ) -> Result<Value, String> {
        let (project, task) = self.tracker_task(task_id)?;
        let saved = self
            .tracker_store()?
            .load_delivery_intent(id)
            .stored()?
            .ok_or("retry_deferred_id: unknown delivery intent")?;
        if saved.task_id != task.id || saved.project_path != task.project_path {
            return Err("retry_deferred_id does not belong to this task".into());
        }
        let target = super::AssignTarget::parse(Some(&saved.target))?;
        self.validate_task_delivery_target(&project, &target)?;
        #[cfg(test)]
        let _author = crate::store::delivery_origins::ActivationAuthor::captured(
            self.mutation_intent_context(),
        );
        let intent = self.tracker_store()?.retry_delivery_intent(id).stored()?;
        self.note_tasks_changed(&project, task_id);
        self.task_delivery_answer(&intent)
    }
}
