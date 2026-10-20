//! Attachment admission keeps identities and generations, never transcript items.
use super::*;

pub(in crate::app) struct AttachmentReadContext {
    scope: ThreadReadContext,
    owners: HashMap<String, Vec<ReadAddress>>,
    generations: HashMap<String, String>,
}

#[derive(Clone)]
pub(in crate::app) struct AttachmentReadFence {
    binding: ThreadReadFence,
    generation: String,
}

impl AttachmentReadContext {
    pub(in crate::app) fn capture(app: &AppState) -> Self {
        let mut context = Self {
            scope: ThreadReadContext::default(),
            owners: HashMap::new(),
            generations: HashMap::new(),
        };
        for owner in app.plans.keys().chain(app.runs.keys()) {
            #[cfg(test)]
            crate::app::observation_reads::capture_metrics::record("thread_owner", owner);
            let Ok(roster) = app.entity_agents(owner) else {
                continue;
            };
            context.scope.admitted.insert(
                owner.clone(),
                roster.iter().map(|agent| agent.id.clone()).collect(),
            );
            context.owners.insert(
                owner.clone(),
                roster.iter().map(ReadAddress::from).collect(),
            );
            for agent in roster.iter() {
                if agent.id == agent.conversation_id() {
                    context
                        .generations
                        .insert(agent.id.clone(), agent.thread.id.clone());
                }
            }
        }
        context
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
            .iter()
            .find(|agent| named.as_ref().is_none_or(|named| agent.agent == *named))
            .ok_or_else(|| missing_agent(named.as_deref()))?;
        let conversation_owner = self
            .owners
            .iter()
            .find_map(|(owner, roster)| {
                roster
                    .iter()
                    .any(|candidate| candidate.agent == agent.conversation)
                    .then(|| owner.clone())
            })
            .ok_or_else(|| missing_conversation(agent))?;
        let generation = self
            .generations
            .get(&agent.conversation)
            .ok_or_else(|| missing_conversation(agent))?;
        guard_binding_values(agent, generation, params)?;
        Ok((
            ThreadReadFence {
                owner: owner.into(),
                agent: agent.agent.clone(),
                conversation_owner,
                conversation: agent.conversation.clone(),
            },
            generation.clone(),
        ))
    }

    pub(in crate::app) fn validate(&self, fence: &AttachmentReadFence) -> Result<(), String> {
        self.scope.validate_fence(&fence.binding)?;
        if let Some(current) = self.generations.get(&fence.binding.conversation) {
            if current != &fence.generation {
                return Err(format!(
                    "stale thread_id {}; current thread is {current}",
                    fence.generation
                ));
            }
        } else {
            return Err(format!(
                "unknown conversation_id: {}",
                fence.binding.conversation
            ));
        }
        let binding = self.owners.get(&fence.binding.owner).and_then(|roster| {
            roster
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
