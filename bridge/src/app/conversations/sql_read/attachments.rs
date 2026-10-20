//! Attachment admission keeps identities and generations, never transcript items.
use super::*;
use im::OrdMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(in crate::app) struct AttachmentReadContext {
    owners: OrdMap<String, Arc<AttachmentOwner>>,
    canonical: OrdMap<String, (String, String)>,
}

pub(in crate::app) struct AttachmentOwner {
    agents: Vec<ReadAddress>,
    generations: Vec<(String, String)>,
}

#[derive(Clone)]
pub(in crate::app) struct AttachmentReadFence {
    binding: ThreadReadFence,
    generation: String,
}

impl AttachmentReadContext {
    pub(in crate::app) fn capture(app: &AppState) -> Self {
        let mut context = Self::default();
        for owner in app.plans.keys().chain(app.runs.keys()) {
            context.apply_owner(owner, &Self::owner(app, owner));
        }
        context
    }

    pub(in crate::app) fn owner(app: &AppState, owner: &str) -> Option<Arc<AttachmentOwner>> {
        #[cfg(test)]
        crate::app::observation_reads::capture_metrics::record("thread_owner", owner);
        let roster = app.entity_agents(owner).ok()?;
        Some(Arc::new(AttachmentOwner {
            agents: roster.iter().map(ReadAddress::from).collect(),
            generations: roster
                .iter()
                .filter(|agent| agent.id == agent.conversation_id())
                .map(|agent| (agent.id.clone(), agent.thread.id.clone()))
                .collect(),
        }))
    }

    pub(in crate::app) fn apply_owner(&mut self, owner: &str, row: &Option<Arc<AttachmentOwner>>) {
        if let Some(old) = self.owners.get(owner) {
            for (agent, _) in &old.generations {
                self.canonical.remove(agent);
            }
        }
        match row {
            Some(row) => {
                for (agent, generation) in &row.generations {
                    self.canonical
                        .insert(agent.clone(), (owner.into(), generation.clone()));
                }
                self.owners.insert(owner.into(), row.clone());
            }
            None => {
                self.owners.remove(owner);
            }
        }
    }

    pub(in crate::app) fn prepare(
        &self,
        owner: &str,
        params: &Value,
    ) -> Result<AttachmentReadFence, String> {
        let (binding, generation) = self.memory_binding(owner, params)?;
        let fence = AttachmentReadFence {
            binding,
            generation,
        };
        self.validate(&fence)?;
        Ok(fence)
    }

    fn memory_binding(
        &self,
        owner: &str,
        params: &Value,
    ) -> Result<(ThreadReadFence, String), String> {
        let roster = self.owners.get(owner).ok_or("unknown id")?;
        let named = named_agent_id(params)?;
        let agent = roster
            .agents
            .iter()
            .find(|agent| named.as_ref().is_none_or(|named| agent.agent == *named))
            .ok_or_else(|| missing_agent(named.as_deref()))?;
        let (conversation_owner, generation) = self
            .canonical
            .get(&agent.conversation)
            .ok_or_else(|| missing_conversation(agent))?;
        guard_binding_values(agent, generation, params)?;
        Ok((
            ThreadReadFence {
                owner: owner.into(),
                agent: agent.agent.clone(),
                conversation_owner: conversation_owner.clone(),
                conversation: agent.conversation.clone(),
            },
            generation.clone(),
        ))
    }

    fn admit(&self, owner: &str, agent: &str) -> Result<(), String> {
        let roster = self.owners.get(owner).ok_or("unknown id")?;
        if !roster.agents.iter().any(|row| row.agent == agent) {
            return Err(format!("unknown agent_id: {agent}"));
        }
        Ok(())
    }

    pub(in crate::app) fn validate(&self, fence: &AttachmentReadFence) -> Result<(), String> {
        self.admit(&fence.binding.owner, &fence.binding.agent)?;
        self.admit(
            &fence.binding.conversation_owner,
            &fence.binding.conversation,
        )?;
        let (_, current) = self
            .canonical
            .get(&fence.binding.conversation)
            .ok_or_else(|| format!("unknown conversation_id: {}", fence.binding.conversation))?;
        if current != &fence.generation {
            return Err(format!(
                "stale thread_id {}; current thread is {current}",
                fence.generation
            ));
        }
        let binding = self.owners.get(&fence.binding.owner).and_then(|roster| {
            roster
                .agents
                .iter()
                .find(|agent| agent.agent == fence.binding.agent)
        });
        if binding.is_none_or(|agent| agent.conversation != fence.binding.conversation) {
            return Err(format!(
                "stale conversation_id {}; agent {} changed binding",
                fence.binding.conversation, fence.binding.agent
            ));
        }
        Ok(())
    }
}
