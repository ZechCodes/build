use super::{named_agent_id, optional_nonempty_string, AppState};
use serde_json::Value;

mod agent_to_agent;
mod attachments;
mod inputs;
mod legacy_delivery;
mod native_delivery;
pub(in crate::app) mod operation_ledger;
mod post;
mod read;
mod senders;

pub(in crate::app) use agent_to_agent::AgentSender;
pub use attachments::ATTACHMENT_MAX_BYTES;
pub(in crate::app) use attachments::{media_mime_hint, mime_hint};
pub(in crate::app) use inputs::{
    append_user_thread_messages, apply_thread_action, parse_thread_inputs, parse_viewing_context,
    with_post_receipt,
};
pub(in crate::app) use read::{thread_cursor, thread_detail, view_thread_detail, ReadReport};

/// One addressed agent and the durable conversation it is bound to.
///
/// The addressed identity drives the harness and settings. The canonical
/// identity drives history storage. Keeping both in one resolved value makes
/// it impossible for a caller to validate one agent and then accidentally
/// read the roster's current primary conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct ConversationAddress {
    pub(in crate::app) entity_id: String,
    pub(in crate::app) agent_id: String,
    pub(in crate::app) conversation_entity_id: String,
    pub(in crate::app) conversation_id: String,
    pub(in crate::app) artifact: crate::thread::ArtifactKind,
}

/// Tell every conversation of one entity which checkout it is about.
///
/// Findability is derived when an item is written, and a path in prose is only
/// indexed when the checkout really has that file — so a conversation that was
/// never located indexes no files at all.
pub(in crate::app) fn locate_conversations(
    agents: &mut crate::agent::AgentRoster,
    checkout: &std::path::Path,
) {
    for agent in agents.iter_mut() {
        agent.thread.set_worktree_root(checkout);
    }
}

impl AppState {
    /// Resolve an optional wire address to one stable conversation binding.
    pub(in crate::app) fn resolve_conversation_address(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
    ) -> Result<ConversationAddress, String> {
        let roster = self.entity_agents(entity_id)?;
        let agent = roster.resolve(agent_id)?;
        let conversation_id = agent.conversation_id().to_string();
        let conversation_entity_id = self.entity_of_agent(&conversation_id).ok_or_else(|| {
            format!(
                "agent {} is bound to missing conversation {}",
                agent.id, conversation_id
            )
        })?;
        self.entity_agents(&conversation_entity_id)?
            .by_id(&conversation_id)
            .ok_or_else(|| format!("unknown conversation_id: {conversation_id}"))?;
        Ok(ConversationAddress {
            entity_id: entity_id.to_string(),
            agent_id: agent.id.clone(),
            conversation_entity_id,
            conversation_id,
            artifact: if self.plans.contains_key(entity_id) {
                crate::thread::ArtifactKind::Plan
            } else {
                crate::thread::ArtifactKind::Diff
            },
        })
    }

    /// Resolve strict wire identity and, when supplied, guard against a stale
    /// conversation binding captured before the request crossed the network.
    pub(in crate::app) fn resolve_conversation_params(
        &self,
        entity_id: &str,
        params: &Value,
    ) -> Result<ConversationAddress, String> {
        let agent_id = named_agent_id(params)?;
        let address = self.resolve_conversation_address(entity_id, agent_id.as_deref())?;
        if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
            if expected != address.conversation_id {
                return Err(format!(
                    "stale conversation_id {expected}; agent {} is bound to {}",
                    address.agent_id, address.conversation_id
                ));
            }
        }
        Ok(address)
    }

    pub(in crate::app) fn conversation_at(
        &self,
        address: &ConversationAddress,
    ) -> Result<&crate::thread::Thread, String> {
        debug_assert!(self
            .entity_agents(&address.entity_id)
            .is_ok_and(|roster| roster.by_id(&address.agent_id).is_some()));
        self.entity_agents(&address.conversation_entity_id)?
            .by_id(&address.conversation_id)
            .map(|agent| &agent.thread)
            .ok_or_else(|| format!("unknown conversation_id: {}", address.conversation_id))
    }
}
