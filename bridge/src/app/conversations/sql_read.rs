//! History reads own a SQLite snapshot, never an application or writer lock.
//! The publication contains only live admission identities when SQL is present.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::agent::{Agent, AgentRoster};
use crate::app::{named_agent_id, optional_nonempty_string, require_str, AppState};
use crate::operation::OperationReceipt;
use crate::store::ReadTransaction;
use crate::thread::Thread;

use super::inputs::required_operation_id;
use super::read::{conversation_owner_param, revision_value};

mod attachments;
mod mcp;
mod pages;
pub(in crate::app) use attachments::{AttachmentOwner, AttachmentReadContext, AttachmentReadFence};
pub(in crate::app) use mcp::{McpHistoryQuery, McpHistoryWork};
use pages::{activity, page};

#[derive(Clone, Default)]
pub(in crate::app) struct ThreadReadContext {
    admitted: HashMap<String, HashSet<String>>,
    memory: Option<MemoryReads>,
}

#[derive(Clone, Default)]
struct MemoryReads {
    owners: HashMap<String, AgentRoster>,
    operations: HashMap<String, OperationReceipt>,
}

struct ReadAddress {
    agent: String,
    conversation: String,
}

impl From<&Agent> for ReadAddress {
    fn from(agent: &Agent) -> Self {
        Self {
            agent: agent.id.clone(),
            conversation: agent.conversation_id().to_string(),
        }
    }
}

#[derive(Clone)]
pub(in crate::app) struct ThreadReadFence {
    owner: String,
    agent: String,
    conversation_owner: String,
    conversation: String,
}

impl AppState {
    pub(in crate::app) fn thread_read_context(&self) -> ThreadReadContext {
        let mut context = ThreadReadContext {
            admitted: HashMap::new(),
            memory: self.store.is_none().then(MemoryReads::default),
        };
        for owner in self.plans.keys().chain(self.runs.keys()) {
            context.update_owner(self, owner);
        }
        context
    }
}

impl ThreadReadContext {
    /// Publish one completed owner without interpreting other temporarily
    /// borrowed owners as deletions.
    pub(in crate::app) fn update_owner(&mut self, app: &AppState, owner: &str) {
        let Ok(roster) = app.entity_agents(owner) else {
            self.forget_owner(owner);
            return;
        };
        self.admitted.insert(
            owner.to_string(),
            roster.iter().map(|agent| agent.id.clone()).collect(),
        );
        if let Some(memory) = &mut self.memory {
            memory.owners.insert(owner.to_string(), roster.clone());
            memory.operations = app.operation_ledger.read_snapshot();
        }
    }

    pub(in crate::app) fn forget_owner(&mut self, owner: &str) {
        self.admitted.remove(owner);
        if let Some(memory) = &mut self.memory {
            memory.owners.remove(owner);
        }
    }

    /// Delivery observations change receipts independently of roster saves.
    /// Keep committed owners and histories even while a producer holds one
    /// outside the live maps or has installed a candidate whose save failed.
    pub(in crate::app) fn refresh_operations(&mut self, app: &AppState) {
        if let Some(memory) = &mut self.memory {
            memory.operations = app.operation_ledger.read_snapshot();
        }
    }

    /// A failed save cannot publish a candidate roster. Actual retirement may
    /// still remove access, without admitting additions or changed bindings.
    pub(in crate::app) fn retain_owner_agents(&mut self, app: &AppState, owner: &str) {
        let Ok(roster) = app.entity_agents(owner) else {
            self.forget_owner(owner);
            return;
        };
        if let Some(admitted) = self.admitted.get_mut(owner) {
            admitted.retain(|agent| roster.by_id(agent).is_some());
        }
        let Some(saved) = self
            .memory
            .as_mut()
            .and_then(|memory| memory.owners.get_mut(owner))
        else {
            return;
        };
        let removed: Vec<_> = saved
            .iter()
            .filter(|agent| roster.by_id(&agent.id).is_none())
            .map(|agent| agent.id.clone())
            .collect();
        for agent in removed {
            saved
                .remove(&agent)
                .expect("retained snapshot contained agent");
        }
    }

    pub(in crate::app) fn validate_fence(&self, fence: &ThreadReadFence) -> Result<(), String> {
        self.admit_agent(&fence.owner, &fence.agent)?;
        self.admit_agent(&fence.conversation_owner, &fence.conversation)
    }

    fn admit_agent(&self, owner: &str, agent: &str) -> Result<(), String> {
        let admitted = self.admitted.get(owner).ok_or("unknown id")?;
        if !admitted.contains(agent) {
            return Err(format!("unknown agent_id: {agent}"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::app) fn read(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        self.read_scoped(transaction, method, params)
            .map(|(value, _)| value)
    }

    pub(in crate::app) fn read_scoped(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        method: &str,
        params: &Value,
    ) -> Result<(Value, ThreadReadFence), String> {
        let owner = if matches!(method, "thread.operation" | "thread.revision") {
            require_str(params, "entity_id")?
        } else {
            conversation_owner_param(params)?
        };
        let revision_id = (method == "thread.revision")
            .then(|| require_str(params, "revision_id"))
            .transpose()?;
        let (mut thread, fence) = self.resolve(transaction, &owner, params)?;
        if let Some(revision_id) = revision_id {
            return revision_value(&thread, &revision_id).map(|value| (value, fence));
        }
        if let Some(transaction) = transaction {
            restore_read_counters(transaction, thread.to_mut(), method)?;
        }
        let mut value = match method {
            "thread.page" => page(transaction, &thread, params),
            "thread.activity" => activity(transaction, &thread, params),
            "thread.operation" => self.operation(transaction, &owner, params),
            _ => Err(format!("unknown method: {method}")),
        }?;
        if matches!(method, "thread.page" | "thread.activity") {
            if let Some(transaction) = transaction {
                let boundary = transaction
                    .thread_retention_boundary(&thread.agent.id)
                    .map_err(conversation_error)?;
                attach_retention_boundary(&mut value, &thread.id, boundary.as_ref());
            }
        }
        Ok((value, fence))
    }

    fn resolve<'a>(
        &'a self,
        transaction: Option<&ReadTransaction<'_>>,
        owner: &str,
        params: &Value,
    ) -> Result<(Cow<'a, Thread>, ThreadReadFence), String> {
        self.admitted.get(owner).ok_or("unknown id")?;
        let named = named_agent_id(params)?;
        let (agent, conversation_owner, thread) = match transaction {
            Some(transaction) => self.resolve_stored(transaction, owner, named.as_deref())?,
            None => self.resolve_memory(owner, named.as_deref())?,
        };
        guard_binding(&agent, &thread, params)?;
        let fence = ThreadReadFence {
            owner: owner.to_string(),
            agent: agent.agent,
            conversation_owner,
            conversation: agent.conversation,
        };
        self.validate_fence(&fence)?;
        Ok((thread, fence))
    }

    fn resolve_stored(
        &self,
        transaction: &ReadTransaction<'_>,
        owner: &str,
        named: Option<&str>,
    ) -> Result<(ReadAddress, String, Cow<'static, Thread>), String> {
        let admitted = &self.admitted[owner];
        let mut roster = transaction.agents(owner).map_err(conversation_error)?;
        let index = roster
            .iter()
            .position(|agent| {
                admitted.contains(&agent.id)
                    && match named {
                        Some(named) => agent.id == named,
                        None => true,
                    }
            })
            .ok_or_else(|| missing_agent(named))?;
        let agent = roster.remove(index);
        let address = ReadAddress::from(&agent);
        let (conversation_owner, mut canonical) =
            canonical_stored_agent(transaction, owner, agent, roster)?;
        if self
            .admit_agent(&conversation_owner, &address.conversation)
            .is_err()
        {
            return Err(missing_conversation(&address));
        }
        canonical.thread.rekey_to_agent(&canonical.id);
        Ok((address, conversation_owner, Cow::Owned(canonical.thread)))
    }

    fn resolve_memory(
        &self,
        owner: &str,
        named: Option<&str>,
    ) -> Result<(ReadAddress, String, Cow<'_, Thread>), String> {
        let memory = self
            .memory
            .as_ref()
            .ok_or("this conversation's history is not stored")?;
        let agent = memory
            .owners
            .get(owner)
            .ok_or("unknown id")?
            .resolve(named)?;
        let (canonical_owner, canonical) = memory
            .owners
            .iter()
            .find_map(|(owner, roster)| {
                roster
                    .by_id(agent.conversation_id())
                    .map(|agent| (owner, agent))
            })
            .ok_or_else(|| missing_conversation(&ReadAddress::from(agent)))?;
        Ok((
            ReadAddress::from(agent),
            canonical_owner.clone(),
            Cow::Borrowed(&canonical.thread),
        ))
    }

    fn operation(
        &self,
        transaction: Option<&ReadTransaction<'_>>,
        owner: &str,
        params: &Value,
    ) -> Result<Value, String> {
        let id = required_operation_id(params)?;
        let receipt = match transaction {
            Some(transaction) => transaction
                .operation(&id)
                .map_err(|error| format!("operation store: {error}"))?,
            None => self
                .memory
                .as_ref()
                .and_then(|memory| memory.operations.get(&id))
                .cloned(),
        }
        .ok_or_else(|| format!("unknown operation_id: {id}"))?;
        if receipt.entity_id != owner {
            return Err("thread.operation: operation does not belong to entity".into());
        }
        if named_agent_id(params)?.is_some_and(|agent| agent != receipt.agent_id) {
            return Err("thread.operation: operation does not belong to agent".into());
        }
        Ok(receipt.wire_value())
    }
}

fn canonical_stored_agent(
    transaction: &ReadTransaction<'_>,
    owner: &str,
    agent: Agent,
    roster: Vec<Agent>,
) -> Result<(String, Agent), String> {
    if agent.conversation_id() == agent.id {
        return Ok((owner.to_string(), agent));
    }
    if let Some(canonical) = roster
        .into_iter()
        .find(|candidate| candidate.id == agent.conversation_id())
    {
        return Ok((owner.to_string(), canonical));
    }
    transaction
        .load_agent_record(agent.conversation_id())
        .map_err(conversation_error)?
        .ok_or_else(|| missing_conversation(&ReadAddress::from(&agent)))
}

fn restore_read_counters(
    transaction: &ReadTransaction<'_>,
    thread: &mut Thread,
    method: &str,
) -> Result<(), String> {
    let count = match method {
        "thread.page" => transaction
            .thread_item_count(&thread.agent.id)
            .map_err(conversation_error)?,
        "thread.activity" => 0,
        _ => return Ok(()),
    };
    let sequence = transaction
        .thread_last_sequence(&thread.agent.id)
        .map_err(conversation_error)?;
    thread.adopt_stored_tail(Vec::new(), count, sequence);
    Ok(())
}

fn missing_agent(named: Option<&str>) -> String {
    named.map_or_else(
        || "no agent here yet — send a message to create one".into(),
        |id| format!("unknown agent_id: {id}"),
    )
}

fn missing_conversation(agent: &ReadAddress) -> String {
    format!(
        "agent {} is bound to missing conversation {}",
        agent.agent, agent.conversation
    )
}

fn conversation_error(error: crate::store::StoreError) -> String {
    format!("conversation store: {error}")
}

fn guard_binding(agent: &ReadAddress, thread: &Thread, params: &Value) -> Result<(), String> {
    guard_binding_values(agent, &thread.id, params)
}

fn guard_binding_values(
    agent: &ReadAddress,
    thread_id: &str,
    params: &Value,
) -> Result<(), String> {
    if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
        if expected != agent.conversation {
            return Err(format!(
                "stale conversation_id {expected}; agent {} is bound to {}",
                agent.agent, agent.conversation
            ));
        }
    }
    if let Some(expected) = params.get("thread_id") {
        let expected = expected
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("invalid thread_id: expected a non-empty string")?;
        if expected != thread_id {
            return Err(format!(
                "stale thread_id {expected}; current thread is {}",
                thread_id
            ));
        }
    }
    Ok(())
}

/// A floor belongs to a generation, so a reset cannot inherit the old trim.
pub(super) fn attach_retention_boundary(
    value: &mut Value,
    thread_id: &str,
    boundary: Option<&crate::store::RetentionBoundary>,
) {
    if let Some(boundary) = boundary.filter(|boundary| boundary.thread_id == thread_id) {
        value["history_start_sequence"] = boundary.history_start_sequence.into();
    }
}
