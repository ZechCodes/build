//! Durable dependency and failure notices. These never grant assignment authority.
use super::dispatch::envelope_of;
use super::StoredAnswer;
use crate::app::AppState;
use crate::store::{DeliveryIntent, DeliveryNotice};
use crate::tracker::{Assignee, Task, TaskEvent, TaskEventKind};
use serde_json::{json, Value};

impl AppState {
    pub(super) fn pump_task_delivery_notices(&mut self) -> Result<bool, String> {
        let Some(mut notice) = self
            .tracker_store()?
            .delivery_notices_ready(1)
            .stored()?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let (project, task) = match self.tracker_task(&notice.task_id) {
            Ok(found) if found.1.project_path == notice.project_path => found,
            _ => {
                self.tracker_store()?
                    .settle_delivery_notice(&notice.id)
                    .stored()?;
                return Ok(true);
            }
        };
        if notice.admitted.is_none() {
            let frozen = self.freeze_task_delivery_notice(&project, &task, &notice)?;
            notice = self
                .tracker_store()?
                .checkpoint_delivery_notice(&notice.id, frozen)
                .stored()?;
        }
        for message in notice
            .admitted
            .as_ref()
            .unwrap()
            .as_array()
            .ok_or("invalid saved dependency notice")?
        {
            self.post_saved_delivery_notice(&project, message)?;
        }
        self.tracker_store()?
            .settle_delivery_notice(&notice.id)
            .stored()?;
        Ok(true)
    }

    fn post_saved_delivery_notice(&mut self, project: &str, message: &Value) -> Result<(), String> {
        let params = &message["params"];
        let Some(agent) = params["agent_id"].as_str() else {
            return Ok(());
        };
        if self
            .live_task_delivery_agent(project, agent)
            .ok()
            .as_deref()
            != params["entity_id"].as_str()
        {
            return Ok(());
        }
        if self
            .operation_receipt(
                params["operation_id"]
                    .as_str()
                    .ok_or("notice has no operation")?,
            )?
            .is_some()
        {
            return Ok(());
        }
        // A reset or retired owner revokes the addressed generation. Do
        // not redirect an old notice to the replacement conversation.
        let address =
            self.resolve_conversation_address(params["entity_id"].as_str().unwrap(), Some(agent))?;
        if Some(self.conversation_at(&address)?.id.as_str()) != params["thread_id"].as_str() {
            return Ok(());
        }
        self.thread_post_frozen_task(message)?;
        Ok(())
    }

    fn freeze_task_delivery_notice(
        &mut self,
        project: &str,
        task: &Task,
        notice: &DeliveryNotice,
    ) -> Result<Value, String> {
        let event: TaskEvent =
            serde_json::from_value(notice.payload.clone()).map_err(|e| e.to_string())?;
        let recipients = self.task_delivery_notice_recipients(project, task, &event)?;
        let body = self.task_delivery_notice_body(task, &event)?;
        let mut messages = Vec::new();
        for agent in recipients {
            let Ok(entity) = self.live_task_delivery_agent(project, &agent) else {
                continue;
            };
            let address = self.resolve_conversation_address(&entity, Some(&agent))?;
            messages.push(json!({
                "params":{"entity_id":entity,"agent_id":agent,"conversation_id":address.conversation_id,
                    "thread_id":self.conversation_at(&address)?.id,"body":body,
                    "operation_id":format!("{}-{}",notice.operation_id,agent)},
                "envelope":envelope_of(task),"sender":null,"requester":null,
            }));
        }
        Ok(json!(messages))
    }

    fn task_delivery_notice_recipients(
        &mut self,
        project: &str,
        task: &Task,
        event: &TaskEvent,
    ) -> Result<Vec<String>, String> {
        if matches!(
            event.kind,
            TaskEventKind::Unblocked | TaskEventKind::DispatchFailed
        ) {
            let owner = self.project_ensure_conversation(&json!({"project_id":project}))?;
            let entity = owner["run_id"].as_str().ok_or("coordinator has no owner")?;
            return Ok(vec![self.ensure_primary_agent(entity)?]);
        }
        let mut recipients = task.trackers.clone();
        let intent = self
            .tracker_store()?
            .current_delivery_intent(&task.id)
            .stored()?;
        if self.task_delivery_notice_holder_is_pending(intent.as_ref())? {
            let holder = self.pending_delivery_holder(project, task, intent.as_ref().unwrap());
            recipients.retain(|agent| Some(agent.as_str()) != holder.as_deref());
            return Ok(recipients);
        }
        if let Some(Assignee::Agent { agent_id }) = &task.assignee {
            if !recipients.contains(agent_id) {
                recipients.push(agent_id.clone());
            }
        }
        if let Some(intent) = intent {
            if let Some(agent) = intent.reservation["agent_id"].as_str() {
                if !recipients.iter().any(|id| id == agent) {
                    recipients.push(agent.into());
                }
            }
        }
        Ok(recipients)
    }

    fn task_delivery_notice_holder_is_pending(
        &self,
        intent: Option<&DeliveryIntent>,
    ) -> Result<bool, String> {
        let Some(intent) = intent else {
            return Ok(false);
        };
        Ok(intent.state != "delivered" && self.operation_receipt(&intent.operation_id)?.is_none())
    }

    fn pending_delivery_holder(
        &self,
        project: &str,
        task: &Task,
        intent: &DeliveryIntent,
    ) -> Option<String> {
        if let Some(agent) = intent.reservation["agent_id"].as_str() {
            return Some(agent.to_string());
        }
        if let Some(agent) = task.assignee.as_ref().and_then(Assignee::agent_id) {
            return Some(agent.to_string());
        }
        if intent.target["kind"] == "project_agent" {
            let entity = self.project_conversation_run(project)?;
            return self.project_pin(&entity).map(str::to_string);
        }
        None
    }

    pub(super) fn task_dependency_release_context(&self, task: &Task) -> Result<String, String> {
        let timeline = self
            .tracker_store()?
            .load_tracker_timeline(&task.id)
            .stored()?;
        let release = timeline.iter().rev()
            .take_while(|entry| !matches!(entry, crate::tracker::TimelineEntry::Event(event) if event.kind == TaskEventKind::Assigned))
            .find_map(|entry| match entry {
                crate::tracker::TimelineEntry::Event(event) if event.kind == TaskEventKind::Unblocked => Some(event),
                _ => None,
            });
        match release {
            Some(event) => self.removed_prerequisite_context(event),
            None => Ok(String::new()),
        }
    }

    fn removed_prerequisite_context(&self, event: &TaskEvent) -> Result<String, String> {
        let Some(ids) = event.payload["removed_task_ids"].as_array() else {
            return Ok(String::new());
        };
        let mut names = Vec::new();
        for id in ids.iter().filter_map(Value::as_str) {
            let name = match self.tracker_store()?.load_tracker_task(id).stored()? {
                Some(task) => format!("#{} — {} ({})", task.number, task.title, id),
                None => id.to_string(),
            };
            names.push(name);
        }
        if names.is_empty() {
            return Ok(String::new());
        }
        Ok(format!(
            "Prerequisite edges were removed: {}. Removing an edge does not mark its task Done.",
            names.join(", ")
        ))
    }

    fn task_delivery_notice_body(&self, task: &Task, event: &TaskEvent) -> Result<String, String> {
        let mut body = match event.kind {
            TaskEventKind::Unblocked => format!("Task #{} — {} ({}) has no unresolved prerequisites. Decide who should take it; read the current task with get_task.", task.number, task.title, task.id),
            TaskEventKind::DispatchFailed => format!("Task #{} — {} ({}) could not be delivered: {}. Read the task with get_task and decide how to recover the saved assignment.", task.number, task.title, task.id, event.payload["error"].as_str().unwrap_or("delivery failed")),
            _ => format!("Task #{} — {} ({}) is blocked again by prerequisites {}. This is informational; it does not dispatch another assignment. Read the current task with get_task.", task.number, task.title, task.id, event.payload["unresolved_task_ids"]),
        };
        if event.kind == TaskEventKind::Unblocked {
            let removed = self.removed_prerequisite_context(event)?;
            if !removed.is_empty() {
                body.push_str("\n\n");
                body.push_str(&removed);
            }
            let context = self.assignment_closure_context(task)?;
            if !context.is_empty() {
                body.push_str("\n\n");
                body.push_str(&context);
            }
        }
        Ok(body)
    }
}
