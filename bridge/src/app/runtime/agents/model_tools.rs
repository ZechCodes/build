use crate::app::{AgentSender, AppState, TabKey};
use crate::harness::{harness_for, AgentStatus, LiveModelChange};
use serde_json::{json, Value};

impl AppState {
    /// Save the picker's next-turn choice, provided the exact live session
    /// can spend it without delivery having to replace that session.
    pub(in crate::app) fn set_agent_model(
        &mut self,
        sender: AgentSender<'_>,
        agent_id: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<Value, String> {
        let owner = self.reachable_agent("set_agent_model", sender, agent_id)?;
        let address = self.resolve_conversation_address(&owner, Some(agent_id))?;
        self.guard_conversation_reservation(&address)?;
        let agent = self.entity_agents(&owner)?.resolve(Some(agent_id))?;
        let provider = agent.choice.provider;
        let key = TabKey::agent(
            &Self::canonical_root(&self.entity_agent_root(&owner)?),
            agent_id,
        );
        let thread = self.conversation_at(&address)?;
        let no_session = || format!("set_agent_model: agent {agent_id} has no running session.");
        let instance = thread
            .open_session_instance(agent_id)
            .ok_or_else(no_session)?;
        let tab = self
            .session_registry
            .agent_snapshot(&key)
            .filter(|tab| tab.live && tab.instance.as_ref() == Some(&instance))
            .ok_or_else(no_session)?;
        let status = tab.session.status();
        if matches!(status, AgentStatus::Ended { .. }) {
            return Err(no_session());
        }
        let launch_effort = thread
            .sessions
            .iter()
            .rev()
            .find(|session| session.id == instance.id)
            .and_then(|session| session.effort.as_deref());
        let changed = harness_for(provider).choose_live_model(LiveModelChange {
            readings: &self.cli_readings,
            session: tab.session.as_ref(),
            status,
            model,
            requested_effort: effort,
            saved_effort: agent.choice.effort.as_deref(),
            launch_effort,
        })?;
        let choice = changed.choice;
        let deferred = changed.deferred || self.delivery_queue.agent_in_flight(&key);
        let choice_revision = self.set_agent_model_choice(&owner, agent_id, choice.clone())?;
        let message = if deferred {
            format!(
                "Deferred agent {agent_id}'s model change to {model} until its next turn starts."
            )
        } else {
            format!("Set agent {agent_id}'s model choice to {model} for newly queued turns.")
        };
        Ok(json!({
            "agent_id": agent_id,
            "provider": provider,
            "model": model,
            "effort": choice.effort,
            "choice_revision": choice_revision,
            "deferred": deferred,
            "message": message,
        }))
    }
}
