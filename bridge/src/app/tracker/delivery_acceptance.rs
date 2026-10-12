use super::dispatch::{AssignTarget, Delivered};
use super::{StoredAnswer, TaskWrite};
use crate::app::git::deferred::DeferredGit;
use crate::app::{AgentSender, AppState};
use crate::store::DeliveryIntent;
use crate::tracker::{Actor, Task};
use serde_json::{json, Value};

impl AssignTarget {
    pub(super) fn wire_target(&self) -> Value {
        let mut value = json!({"kind": self.wire_kind()});
        match self {
            Self::Nobody => return Value::Null,
            Self::Agent { agent_id } => value["agent_id"] = json!(agent_id),
            Self::NewWorkspace {
                name,
                isolation,
                choice,
                notify_user,
                agent_name,
            } => {
                value
                    .as_object_mut()
                    .unwrap()
                    .extend(choice.args().wire().as_object().unwrap().clone());
                insert_some(&mut value, "name", name);
                insert_some(&mut value, "isolation", isolation);
                insert_some(&mut value, "notify_user", notify_user);
                insert_some(&mut value, "agent_name", agent_name);
            }
            Self::NewAgent {
                workspace_id,
                choice,
                notify_user,
                agent_name,
            } => {
                value["workspace_id"] = json!(workspace_id);
                value
                    .as_object_mut()
                    .unwrap()
                    .extend(choice.args().wire().as_object().unwrap().clone());
                insert_some(&mut value, "notify_user", notify_user);
                insert_some(&mut value, "agent_name", agent_name);
            }
            Self::NewProjectAgent {
                choice,
                notify_user,
                agent_name,
            } => {
                value
                    .as_object_mut()
                    .unwrap()
                    .extend(choice.args().wire().as_object().unwrap().clone());
                insert_some(&mut value, "notify_user", notify_user);
                insert_some(&mut value, "agent_name", agent_name);
            }
            _ => {}
        }
        value
    }
}

fn insert_some<T: serde::Serialize>(value: &mut Value, key: &str, field: &Option<T>) {
    if let Some(field) = field {
        value[key] = json!(field);
    }
}

impl AppState {
    /// Record every agent assignment before creating resources or admitting text.
    pub(super) fn accept_task_delivery(
        &mut self,
        project: &str,
        task: Task,
        target: AssignTarget,
        handoff: super::handoff::Handoff,
        actor: Actor,
        sender: Option<AgentSender<'_>>,
    ) -> Result<Value, String> {
        let saved_sender = self.saved_delivery_sender(project, sender)?;
        if let Some(existing) =
            self.replayed_delivery(&task, &target, &handoff, &actor, &saved_sender)?
        {
            return self.task_delivery_answer(&existing);
        }
        self.validate_task_delivery_target(project, &target)?;
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(actor.clone(), task);
        let facts = self
            .tracker_store()?
            .waiting_on_summary(&write.task.project_path, &write.task.id)
            .stored()?;
        if facts.blocked && matches!(write.task.status.as_str(), "in_progress" | "in_review") {
            super::edits::move_to(
                &mut write,
                "ready",
                &actor,
                json!({"by":"assignment"}),
                &now,
            );
        }
        write.task.assignee = target.settled_assignee().flatten();
        write.event(
            &actor,
            crate::tracker::TaskEventKind::Assigned,
            json!({"assignee":write.task.assignee,"target":target.wire_target()}),
            &now,
        );
        write.task.updated_at = now;
        let intent = DeliveryIntent {
            id: String::new(),
            task_id: write.task.id.clone(),
            project_path: write.task.project_path.clone(),
            generation: 0,
            target: target.wire_target(),
            actor,
            sender: saved_sender,
            note: handoff.note,
            operation_id: handoff.operation_id.unwrap_or_default(),
            state: String::new(),
            reservation: Value::Null,
            admitted: None,
            error: None,
        };
        let intent = self
            .tracker_store()?
            .accept_delivery_intent(&write.task, &write.events, intent)
            .stored()?;
        self.note_tasks_changed(project, &intent.task_id);
        if intent.state == "waiting" {
            return self.task_delivery_answer(&intent);
        }
        self.start_immediate_task_delivery(intent)
    }

    fn saved_delivery_sender(
        &self,
        project: &str,
        sender: Option<AgentSender<'_>>,
    ) -> Result<Option<Value>, String> {
        sender.map(|sender| {
            if self.agent_of_this_project(project, sender.agent_id)? != sender.entity_id {
                return Err("task assigner belongs to another conversation".into());
            }
            let generation = self.mcp_conversation_generation(sender.entity_id, sender.agent_id)?;
            Ok(json!({"entity_id":generation.entity_id,"agent_id":generation.agent_id,"thread_id":generation.thread_id}))
        }).transpose()
    }

    fn replayed_delivery(
        &self,
        task: &Task,
        target: &AssignTarget,
        handoff: &super::handoff::Handoff,
        actor: &Actor,
        sender: &Option<Value>,
    ) -> Result<Option<DeliveryIntent>, String> {
        let Some(operation) = handoff.operation_id.as_deref() else {
            return Ok(None);
        };
        let existing = self
            .tracker_store()?
            .delivery_intent_for_operation(operation)
            .stored()?;
        if let Some(existing) = &existing {
            if existing.task_id != task.id
                || existing.project_path != task.project_path
                || existing.target != target.wire_target()
                || existing.note != handoff.note
                || existing.actor != *actor
                || existing.sender != *sender
            {
                return Err("operation_id already used with different assignment".into());
            }
        }
        Ok(existing)
    }

    fn start_immediate_task_delivery(&mut self, intent: DeliveryIntent) -> Result<Value, String> {
        let Some(mut claimed) = self
            .tracker_store()?
            .claim_delivery_intent(&intent.id, intent.generation)
            .stored()?
        else {
            return self.task_delivery_answer(&intent);
        };
        let git = match self.prepare_delivery_resource(&mut claimed) {
            Ok(git) => git,
            Err(error) => {
                self.fail_task_delivery(&claimed, &error)?;
                return Err(error);
            }
        };
        self.task_delivery_in_flight.insert(claimed.id.clone());
        if let Some(git) = git {
            let pending = json!({"task_id":claimed.task_id,"workspace_id":claimed.reservation["workspace"]["id"],"pending":true});
            self.deferred_work = Some(crate::app::DeferredWork::Git(Box::new(DeferredGit {
                call: Box::new(ImmediateTaskDelivery {
                    git,
                    intent: claimed,
                }),
                params: Value::Null,
                invalidates: false,
                #[cfg(test)]
                gate: None,
            })));
            return Ok(pending);
        }
        self.finish_task_delivery(claimed.clone(), None, None)?;
        self.completed_task_delivery_answer(&claimed.id)
    }

    fn completed_task_delivery_answer(&mut self, id: &str) -> Result<Value, String> {
        let updated = self
            .tracker_store()?
            .load_delivery_intent(id)
            .stored()?
            .ok_or("delivery disappeared")?;
        if let Some(error) = &updated.error {
            return Err(error.clone());
        }
        self.task_delivery_answer(&updated)
    }

    fn task_delivery_answer(&mut self, intent: &DeliveryIntent) -> Result<Value, String> {
        let (_, task) = self.tracker_task(&intent.task_id)?;
        let mut answer = self.tasks_get(&json!({"task_id":task.id}))?;
        let dispatch = if intent.state == "delivered" {
            let receipt = self
                .operation_receipt(&intent.operation_id)?
                .ok_or("delivery has no receipt")?;
            Some(
                Delivered {
                    workspace_id: intent.reservation["workspace_id"]
                        .as_str()
                        .map(str::to_string),
                    entity_id: receipt.entity_id,
                    agent_id: receipt.agent_id,
                    operation_id: intent.operation_id.clone(),
                }
                .wire(intent.target["kind"].as_str().unwrap_or("agent")),
            )
        } else {
            None
        };
        answer["dispatch"] = json!(dispatch);
        if matches!(
            intent.state.as_str(),
            "waiting" | "queued" | "provisioning" | "failed"
        ) {
            answer["task"]["deferred_assignment"] = json!({"id":intent.id,"target":intent.target,"state":intent.state,"error":intent.error});
        }
        Ok(answer)
    }

    /// Internal recovery entrypoint; the wire retry surface belongs to W3.
    #[cfg(test)]
    pub(in crate::app) fn retry_task_delivery(&mut self, id: &str) -> Result<(), String> {
        self.tracker_store()?.retry_delivery_intent(id).stored()?;
        Ok(())
    }
}

struct ImmediateTaskDelivery {
    git: DeferredGit,
    intent: DeliveryIntent,
}

impl crate::app::git::deferred::DeferredGitWork for ImmediateTaskDelivery {
    fn run(&self, _params: &Value) -> Result<Value, String> {
        Ok(match self.git.run() {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error}),
        })
    }
    fn invalidate(&self, _app: &mut AppState) {}
    fn settle(&self, app: &mut AppState, result: Value) -> Result<Value, String> {
        let result = match result["error"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(result["value"].clone()),
        };
        app.finish_task_delivery(self.intent.clone(), Some(&self.git), Some(result))?;
        app.completed_task_delivery_answer(&self.intent.id)
    }
    fn invalidates_on_error(&self) -> bool {
        true
    }
}
