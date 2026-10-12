//! Durable task hand-offs. Each checkpoint preserves identities across restart.
use super::dispatch::{assignment_notice, envelope_of, AssignTarget, Delivered};
use super::{StoredAnswer, TaskWrite};
use crate::app::git::deferred::DeferredGit;
use crate::app::{AgentSender, AppState};
use crate::store::DeliveryIntent;
use crate::tracker::{Actor, Task};
use serde_json::{json, Value};

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DeferredDeliveryBoundary {
    Reserved,
    Materialized,
    Agent,
    Frozen,
    Posted,
    Settled,
}

#[cfg(test)]
thread_local! {
    static STOP_AFTER: std::cell::Cell<Option<DeferredDeliveryBoundary>> = const { std::cell::Cell::new(None) };
}

macro_rules! stop_after {
    ($boundary:ident) => {{
        #[cfg(test)]
        {
            let stopped = STOP_AFTER.with(|stop| {
                if stop.get() == Some(DeferredDeliveryBoundary::$boundary) {
                    stop.set(None);
                    true
                } else {
                    false
                }
            });
            if stopped {
                return Ok(());
            }
        }
    }};
}

pub(super) struct TaskDeliveryWork {
    pub intent: DeliveryIntent,
    pub git: Option<DeferredGit>,
}

impl AppState {
    #[cfg(test)]
    pub(in crate::app) fn stop_task_delivery_after_for_test(
        &self,
        boundary: DeferredDeliveryBoundary,
    ) {
        STOP_AFTER.with(|stop| stop.set(Some(boundary)));
    }

    #[cfg(test)]
    pub(in crate::app) fn pump_task_deliveries_for_test(&mut self) -> Result<bool, String> {
        let page = self.tracker_store()?.reconcile_waiting_on(100).stored()?;
        let noticed = self.pump_task_delivery_notices()?;
        let Some(work) = self.prepare_task_delivery()? else {
            return Ok(page.pending || noticed);
        };
        let result = work.git.as_ref().map(DeferredGit::run);
        self.finish_task_delivery(work.intent, work.git.as_ref(), result)?;
        Ok(true)
    }

    /// Claim a single item, retaining the job outside the RPC deferred slot.
    pub(super) fn prepare_task_delivery(&mut self) -> Result<Option<TaskDeliveryWork>, String> {
        let ready = self
            .tracker_store()?
            .delivery_intents_ready(100)
            .stored()?
            .into_iter()
            .find(|intent| !self.task_delivery_in_flight.contains(&intent.id));
        let Some(ready) = ready else {
            return Ok(None);
        };
        let Some(mut intent) = self
            .tracker_store()?
            .claim_delivery_intent(&ready.id, ready.generation)
            .stored()?
        else {
            return Ok(None);
        };
        if self.operation_receipt(&intent.operation_id)?.is_some() {
            self.task_delivery_in_flight.insert(intent.id.clone());
            return Ok(Some(TaskDeliveryWork { intent, git: None }));
        }
        match self.prepare_delivery_resource(&mut intent) {
            Ok(git) => {
                self.task_delivery_in_flight.insert(intent.id.clone());
                Ok(Some(TaskDeliveryWork { intent, git }))
            }
            Err(error) => {
                self.fail_task_delivery(&intent, &error)?;
                Ok(None)
            }
        }
    }

    pub(super) fn prepare_delivery_resource(
        &mut self,
        intent: &mut DeliveryIntent,
    ) -> Result<Option<DeferredGit>, String> {
        let (project, task, target) = self.guard_task_delivery(intent)?;
        if !matches!(target, AssignTarget::NewWorkspace { .. })
            || intent.reservation["materialized"] == true
        {
            return Ok(None);
        }
        if intent.reservation.get("workspace").is_none() {
            let saved =
                self.reserve_deferred_workspace(&project, &target, &task.title, &intent.actor)?;
            if intent.reservation.is_null() {
                intent.reservation = json!({});
            }
            intent.reservation["workspace"] = saved;
            self.checkpoint_task_delivery(intent)?;
        }
        #[cfg(test)]
        if STOP_AFTER.with(|stop| stop.get() == Some(DeferredDeliveryBoundary::Reserved)) {
            STOP_AFTER.with(|stop| stop.set(None));
            return Ok(None);
        }
        self.guard_task_delivery(intent)?;
        self.prepare_deferred_workspace(&intent.reservation["workspace"])
            .map(Some)
    }

    pub(super) fn finish_task_delivery(
        &mut self,
        mut intent: DeliveryIntent,
        git: Option<&DeferredGit>,
        result: Option<Result<Value, String>>,
    ) -> Result<(), String> {
        let id = intent.id.clone();
        let finished = self.finish_delivery_steps(&mut intent, git, result);
        self.task_delivery_in_flight.remove(&id);
        if let Err(error) = finished {
            self.fail_task_delivery(&intent, &error)?;
        }
        Ok(())
    }

    fn finish_delivery_steps(
        &mut self,
        intent: &mut DeliveryIntent,
        git: Option<&DeferredGit>,
        result: Option<Result<Value, String>>,
    ) -> Result<(), String> {
        if let Some(result) = result {
            let git = git.expect("a resource result has its job");
            let current = self.guard_task_delivery(intent);
            if let Err(error) = current {
                if git.invalidates || git.call.invalidates_on_error() {
                    git.call.invalidate(self);
                }
                return Err(error);
            }
            let settled = result.and_then(|value| git.call.settle(self, value));
            if git.invalidates || git.call.invalidates_on_error() {
                git.call.invalidate(self);
            }
            settled?;
            if !self
                .tracker_store()?
                .delivery_authority_current(&intent.id, intent.generation)
                .stored()?
            {
                return Ok(());
            }
            intent.reservation["materialized"] = json!(true);
            self.checkpoint_task_delivery(intent)?;
            stop_after!(Materialized);
        }
        if !self
            .tracker_store()?
            .delivery_intent_is_current(&intent.id, intent.generation)
            .stored()?
            && self.operation_receipt(&intent.operation_id)?.is_none()
        {
            return Ok(());
        }
        self.advance_task_delivery(intent)
    }

    fn advance_task_delivery(&mut self, intent: &mut DeliveryIntent) -> Result<(), String> {
        if let Some(receipt) = self.operation_receipt(&intent.operation_id)? {
            return self.settle_task_delivery(intent, &receipt.entity_id, &receipt.agent_id);
        }
        let (project, task, target) = self.guard_task_delivery(intent)?;
        if matches!(target, AssignTarget::NewWorkspace { .. })
            && intent.reservation["materialized"] != true
        {
            return Ok(());
        }
        if intent.reservation.get("agent_id").is_none() {
            self.reserve_delivery_agent(intent, &project, &target)?;
            stop_after!(Agent);
        }
        self.guard_task_delivery(intent)?;
        if intent.admitted.is_none() {
            intent.admitted = Some(self.freeze_task_delivery(intent, &task)?);
            self.checkpoint_task_delivery(intent)?;
            stop_after!(Frozen);
        }
        self.guard_task_delivery(intent)?;
        self.thread_post_frozen_task(intent.admitted.as_ref().unwrap())?;
        stop_after!(Posted);
        let receipt = self
            .operation_receipt(&intent.operation_id)?
            .ok_or("task delivery has no admission receipt")?;
        self.settle_task_delivery(intent, &receipt.entity_id, &receipt.agent_id)?;
        stop_after!(Settled);
        Ok(())
    }

    fn reserve_delivery_agent(
        &mut self,
        intent: &mut DeliveryIntent,
        project: &str,
        target: &AssignTarget,
    ) -> Result<(), String> {
        let workspace = intent.reservation["workspace"]["id"].as_str();
        let (agent, entity, workspace) =
            self.deferred_assignment_agent(project, target, workspace, &intent.actor, &intent.id)?;
        if intent.reservation.is_null() {
            intent.reservation = json!({});
        }
        intent.reservation["agent_id"] = json!(agent);
        intent.reservation["entity_id"] = json!(entity);
        intent.reservation["workspace_id"] = json!(workspace);
        self.checkpoint_task_delivery(intent)
    }

    fn checkpoint_task_delivery(&self, intent: &DeliveryIntent) -> Result<(), String> {
        self.tracker_store()?
            .checkpoint_delivery_intent(
                &intent.id,
                intent.generation,
                intent.reservation.clone(),
                intent.admitted.clone(),
            )
            .stored()?;
        Ok(())
    }

    fn guard_task_delivery(
        &mut self,
        intent: &DeliveryIntent,
    ) -> Result<(String, Task, AssignTarget), String> {
        if !self
            .tracker_store()?
            .delivery_intent_is_current(&intent.id, intent.generation)
            .stored()?
        {
            return Err("task delivery is no longer ready or current".into());
        }
        let (project, task) = self.tracker_task(&intent.task_id)?;
        if self.tracker_project_path(&project)? != intent.project_path {
            return Err("task delivery project changed".into());
        }
        self.guard_delivery_sender(intent, &project)?;
        let target = AssignTarget::parse(Some(&intent.target))?;
        self.validate_task_delivery_target(&project, &target)?;
        if let Some(agent) = intent.reservation["agent_id"].as_str() {
            let entity = self.live_task_delivery_agent(&project, agent)?;
            if intent.reservation["entity_id"].as_str() != Some(&entity) {
                return Err("task delivery target changed".into());
            }
        }
        Ok((project, task, target))
    }

    pub(super) fn validate_task_delivery_target(
        &self,
        project: &str,
        target: &AssignTarget,
    ) -> Result<(), String> {
        self.tracker_project_path(project)?;
        match target {
            AssignTarget::Agent { agent_id } => {
                self.live_task_delivery_agent(project, agent_id)?;
            }
            AssignTarget::NewAgent { workspace_id, .. } => {
                let workspace = self
                    .workspaces
                    .get(workspace_id)
                    .ok_or("task delivery workspace was removed")?;
                if workspace.project_id != project {
                    return Err("task delivery workspace is in another project".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn live_task_delivery_agent(
        &self,
        project: &str,
        agent: &str,
    ) -> Result<String, String> {
        let entity = self.agent_of_this_project(project, agent)?;
        let owner = self
            .runs
            .get(&entity)
            .ok_or("task delivery owner was removed")?;
        if owner.run.state.is_terminal() {
            return Err("task delivery owner was retired".into());
        }
        Ok(entity)
    }

    fn guard_delivery_sender(&self, intent: &DeliveryIntent, project: &str) -> Result<(), String> {
        if let Some(saved) = &intent.sender {
            let entity = saved["entity_id"].as_str().ok_or("invalid saved sender")?;
            let agent = saved["agent_id"].as_str().ok_or("invalid saved sender")?;
            if intent.actor.agent_id() != Some(agent) {
                return Err("saved sender does not match assignment actor".into());
            }
            if self.live_task_delivery_agent(project, agent)? != entity {
                return Err("task assigner no longer belongs to project".into());
            }
            let generation = crate::app::mcp::McpConversationGeneration {
                entity_id: entity.into(),
                agent_id: agent.into(),
                thread_id: saved["thread_id"]
                    .as_str()
                    .ok_or("invalid saved sender generation")?
                    .into(),
            };
            generation.guard(self)?;
        } else if matches!(intent.actor, Actor::Agent { .. }) {
            return Err("task assigner has no saved authority".into());
        }
        Ok(())
    }

    fn freeze_task_delivery(&self, intent: &DeliveryIntent, task: &Task) -> Result<Value, String> {
        let entity = intent.reservation["entity_id"]
            .as_str()
            .ok_or("task delivery has no owner")?;
        let agent = intent.reservation["agent_id"]
            .as_str()
            .ok_or("task delivery has no agent")?;
        let address = self.resolve_conversation_address(entity, Some(agent))?;
        let sender = intent.sender.as_ref().map(|saved| AgentSender {
            entity_id: saved["entity_id"].as_str().unwrap(),
            agent_id: saved["agent_id"].as_str().unwrap(),
        });
        let mut body = assignment_notice(task, &self.assigner_name(sender), intent.note.as_deref());
        body.push_str("\n\nRead the current task with get_task before acting.");
        let closure = self.assignment_closure_context(task)?;
        if !closure.is_empty() {
            body.push_str("\n\n");
            body.push_str(&closure);
        }
        let requester = sender
            .map(|sender| self.agent_requester(sender))
            .transpose()?;
        let identity = sender.map(|sender| self.sender_identity(sender.entity_id, sender.agent_id));
        Ok(json!({
            "params": {"entity_id":entity,"agent_id":agent,"conversation_id":address.conversation_id,
                "thread_id":self.conversation_at(&address)?.id,"body":body,"operation_id":intent.operation_id},
            "envelope":envelope_of(task), "sender":identity,"requester":requester,
            "intent_id":intent.id,"generation":intent.generation,
        }))
    }

    fn settle_task_delivery(
        &mut self,
        intent: &DeliveryIntent,
        entity: &str,
        agent: &str,
    ) -> Result<(), String> {
        let (project, task) = self.tracker_task(&intent.task_id)?;
        let target = AssignTarget::parse(Some(&intent.target))?;
        let delivered = Delivered {
            workspace_id: intent.reservation["workspace_id"]
                .as_str()
                .map(str::to_string),
            entity_id: entity.into(),
            agent_id: agent.into(),
            operation_id: intent.operation_id.clone(),
        };
        let now = crate::store::now_rfc3339();
        let mut write = TaskWrite::by(intent.actor.clone(), task);
        self.settle_assignment(&mut write, &target, &Some(delivered), &intent.actor, &now)?;
        // The creating target resolves only at settlement. Concrete targets
        // were already assigned when authority was accepted.
        if target.settled_assignee().is_some() {
            write
                .events
                .retain(|e| e.kind != crate::tracker::TaskEventKind::Assigned);
        }
        write.task.updated_at = now;
        self.capture_task_identities(
            &mut write.task,
            &write
                .events
                .iter()
                .cloned()
                .map(crate::tracker::TimelineEntry::Event)
                .collect::<Vec<_>>(),
        );
        if self
            .tracker_store()?
            .settle_delivery_intent(&intent.id, intent.generation, &write.task, &write.events)
            .stored()?
        {
            self.note_tasks_changed(&project, &intent.task_id);
            self.capture_task_completion_authority(
                entity,
                agent,
                &intent.task_id,
                &intent.id,
                intent.generation,
            );
        }
        Ok(())
    }

    pub(super) fn fail_task_delivery(
        &self,
        intent: &DeliveryIntent,
        error: &str,
    ) -> Result<(), String> {
        if !self
            .tracker_store()?
            .delivery_intent_is_current(&intent.id, intent.generation)
            .stored()?
        {
            return Ok(());
        }
        self.tracker_store()?
            .fail_delivery_intent(&intent.id, intent.generation, error)
            .stored()?;
        if let Some(project) = self
            .projects
            .iter()
            .find(|p| p.repo_path.to_str() == Some(&intent.project_path))
        {
            self.note_tasks_changed(&project.id, &intent.task_id);
        }
        Ok(())
    }
}
