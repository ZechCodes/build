//! A Complete belongs to a consumed assignment generation, never just an agent.

use crate::app::AppState;
use crate::tracker::{Assignee, Task};

#[derive(Clone)]
pub(in crate::app) struct TaskCompletionAuthority {
    task_id: String,
    entity_id: String,
    conversation_id: String,
    thread_id: String,
    intent_id: String,
    generation: u64,
    operation_id: String,
}

pub(in crate::app) struct PendingTaskCompletionAuthority {
    authority: TaskCompletionAuthority,
    occupied_working_since: Option<String>,
    consumed: bool,
}

impl AppState {
    /// Remember which assignment the queued turn will be for. An occupied
    /// turn keeps its old generation until the replacement is actually read.
    pub(in crate::app) fn capture_task_completion_authority(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        task_id: &str,
        intent_id: &str,
        generation: u64,
    ) {
        let Some(authority) = self.task_completion_for_intent(entity_id, agent_id, intent_id)
        else {
            return;
        };
        if authority.task_id != task_id || authority.generation != generation {
            return;
        }
        if self
            .task_completion_authority
            .get(agent_id)
            .is_some_and(|active| active.operation_id == authority.operation_id)
        {
            return;
        }
        if self
            .pending_task_completion_authority
            .get(agent_id)
            .is_some_and(|pending| {
                pending.authority.operation_id == authority.operation_id || pending.consumed
            })
        {
            return;
        }
        let occupied_working_since = self.task_completion_working_since(entity_id, agent_id);
        self.pending_task_completion_authority.insert(
            agent_id.to_string(),
            PendingTaskCompletionAuthority {
                authority,
                occupied_working_since,
                consumed: false,
            },
        );
    }

    /// Called only after a successful Sent write or an ownership-checked Seen
    /// receipt. The durable operation lets the same path restore authority
    /// after restart without trusting an arbitrary native token.
    pub(in crate::app) fn consume_task_completion_operation(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        operation_id: &str,
    ) {
        self.record_consumed_failure_operation(entity_id, agent_id, operation_id);
        if self
            .spent_task_completion_operations
            .get(agent_id)
            .is_some_and(|spent| spent.contains(operation_id))
        {
            return;
        }
        if self
            .task_completion_authority
            .get(agent_id)
            .is_some_and(|active| active.operation_id == operation_id)
        {
            return;
        }
        let intent = self.store.as_ref().and_then(|store| {
            store
                .delivery_intent_for_operation(operation_id)
                .ok()
                .flatten()
        });
        let Some(intent) = intent else { return };
        if self
            .task_completion_for_intent(entity_id, agent_id, &intent.id)
            .is_none()
        {
            return;
        }
        let reconstructing = !self
            .pending_task_completion_authority
            .contains_key(agent_id)
            && !self.task_completion_authority.contains_key(agent_id);
        // Only actual consumption may replace a different consumed pending
        // turn. Merely accepting another assignment cannot overwrite it.
        if self
            .pending_task_completion_authority
            .get(agent_id)
            .is_some_and(|pending| pending.authority.operation_id != operation_id)
        {
            self.pending_task_completion_authority.remove(agent_id);
        }
        self.capture_task_completion_authority(
            entity_id,
            agent_id,
            &intent.task_id,
            &intent.id,
            intent.generation,
        );
        let working_since = self.task_completion_working_since(entity_id, agent_id);
        let Some(pending) = self.pending_task_completion_authority.get_mut(agent_id) else {
            return;
        };
        if pending.authority.operation_id != operation_id {
            return;
        }
        if reconstructing {
            pending.occupied_working_since = None;
        }
        pending.consumed = true;
        let occupied = pending.occupied_working_since.as_ref();
        if occupied.is_none() || occupied != working_since.as_ref() {
            self.promote_task_completion_authority(agent_id);
        }
    }

    /// A new execution interval may consume a message that was Seen while the
    /// preceding interval was still Working. Repeated Working snapshots are
    /// deliberately not a boundary.
    pub(in crate::app) fn begin_task_completion_turn(&mut self, agent_id: &str) {
        let consumed = self
            .pending_task_completion_authority
            .get(agent_id)
            .is_some_and(|pending| pending.consumed);
        if consumed {
            self.promote_task_completion_authority(agent_id);
        }
    }

    fn promote_task_completion_authority(&mut self, agent_id: &str) {
        let Some(pending) = self.pending_task_completion_authority.remove(agent_id) else {
            return;
        };
        self.dispatched_task
            .insert(agent_id.to_string(), pending.authority.task_id.clone());
        self.task_completion_authority
            .insert(agent_id.to_string(), pending.authority);
    }

    fn task_completion_working_since(&self, entity_id: &str, agent_id: &str) -> Option<String> {
        self.entity_agents(entity_id)
            .ok()?
            .by_id(agent_id)?
            .working_since
            .clone()
    }

    fn task_completion_for_intent(
        &self,
        entity_id: &str,
        agent_id: &str,
        intent_id: &str,
    ) -> Option<TaskCompletionAuthority> {
        let project = self.project_of(entity_id).ok()?;
        if self.live_task_delivery_agent(&project, agent_id).ok()? != entity_id {
            return None;
        }
        let store = self.store.as_ref()?;
        let intent = store.load_delivery_intent(intent_id).ok()??;
        if self.tracker_project_path(&project).ok()? != intent.project_path {
            return None;
        }
        if !store
            .delivery_authority_current(intent_id, intent.generation)
            .ok()?
        {
            return None;
        }
        let receipt = self.operation_receipt(&intent.operation_id).ok()??;
        let address = self
            .resolve_conversation_address(entity_id, Some(agent_id))
            .ok()?;
        let thread_id = intent.admitted.as_ref()?["params"]["thread_id"].as_str()?;
        if receipt.method != crate::operation::THREAD_POST_METHOD
            || receipt.entity_id != entity_id
            || receipt.agent_id != agent_id
            || receipt.conversation_id != address.conversation_id
            || self.conversation_at(&address).ok()?.id != thread_id
            || receipt.delivery_message_span().is_none()
        {
            return None;
        }
        Some(TaskCompletionAuthority {
            task_id: intent.task_id,
            entity_id: entity_id.to_string(),
            conversation_id: receipt.conversation_id,
            thread_id: thread_id.to_string(),
            intent_id: intent.id,
            generation: intent.generation,
            operation_id: intent.operation_id,
        })
    }

    /// Taking the active marker consumes only this turn's authority; a queued
    /// replacement survives an old Complete and may start the next turn.
    pub(in crate::app) fn take_task_completion_authority(
        &mut self,
        entity_id: &str,
        agent_id: &str,
    ) -> Option<Task> {
        self.dispatched_task.remove(agent_id);
        let authority = self.task_completion_authority.remove(agent_id)?;
        self.spent_task_completion_operations
            .entry(agent_id.to_string())
            .or_default()
            .insert(authority.operation_id.clone());
        if authority.entity_id != entity_id {
            return None;
        }
        let project = self.project_of(entity_id).ok()?;
        if self.live_task_delivery_agent(&project, agent_id).ok()? != entity_id {
            return None;
        }
        let address = self
            .resolve_conversation_address(entity_id, Some(agent_id))
            .ok()?;
        if authority.conversation_id != address.conversation_id
            || authority.thread_id != self.conversation_at(&address).ok()?.id
        {
            return None;
        }
        let store = self.store.as_ref()?;
        if !store
            .delivery_authority_current(&authority.intent_id, authority.generation)
            .ok()?
        {
            return None;
        }
        let task = store.load_tracker_task(&authority.task_id).ok()??;
        self.task_is_held_by(&task, entity_id, agent_id)
            .then_some(task)
    }

    pub(in crate::app) fn task_failure_operation(&self, agent_id: &str) -> Option<&str> {
        self.task_completion_authority
            .get(agent_id)
            .map(|authority| authority.operation_id.as_str())
    }

    /// A delayed failure names its own consumed operation. Newer turns may
    /// already have promoted their authority, and must survive this handoff.
    pub(in crate::app) fn take_task_failure_authority(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        operation_id: &str,
    ) -> Option<Task> {
        if self
            .spent_task_completion_operations
            .get(agent_id)
            .is_some_and(|spent| spent.contains(operation_id))
        {
            return None;
        }
        let task = self.task_for_failed_operation(entity_id, agent_id, operation_id);
        self.spent_task_completion_operations
            .entry(agent_id.into())
            .or_default()
            .insert(operation_id.into());
        if self.task_failure_operation(agent_id) == Some(operation_id) {
            self.task_completion_authority.remove(agent_id);
            self.dispatched_task.remove(agent_id);
        }
        if self
            .pending_task_completion_authority
            .get(agent_id)
            .is_some_and(|pending| pending.authority.operation_id == operation_id)
        {
            self.pending_task_completion_authority.remove(agent_id);
        }
        task
    }

    pub(in crate::app) fn task_for_failed_operation(
        &self,
        entity_id: &str,
        agent_id: &str,
        operation_id: &str,
    ) -> Option<Task> {
        let intent = self
            .store
            .as_ref()?
            .delivery_intent_for_operation(operation_id)
            .ok()??;
        let authority = self.task_completion_for_intent(entity_id, agent_id, &intent.id)?;
        let task = self
            .store
            .as_ref()?
            .load_tracker_task(&authority.task_id)
            .ok()??;
        self.task_is_held_by(&task, entity_id, agent_id)
            .then_some(task)
    }

    fn task_is_held_by(&self, task: &Task, entity_id: &str, agent_id: &str) -> bool {
        match task.assignee.as_ref() {
            Some(Assignee::Agent { agent_id: holder }) => holder == agent_id,
            Some(Assignee::ProjectAgent) => self.is_project_coordinator(entity_id, agent_id),
            _ => false,
        }
    }
}
