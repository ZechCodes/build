use crate::app::{AgentSender, AppState, TabKey};
use crate::harness::installed::{model_offer_from, Readings};
use crate::harness::{harness_for, AgentStatus, TurnChoiceSupport};
use crate::models::{AgentProvider, ModelChoice};
use serde_json::{json, Value};
use std::sync::Arc;

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
        let cannot_change = || {
            format!(
                "set_agent_model: {} cannot change model in place on this session.",
                provider.label()
            )
        };
        if !tab.session.accepts_turn_choice() {
            return Err(cannot_change());
        }

        let existing_effort = if provider == AgentProvider::ClaudeAdk {
            thread
                .sessions
                .iter()
                .find(|session| session.id == instance.id)
                .and_then(|session| session.effort.as_deref())
        } else {
            agent.choice.effort.as_deref()
        };
        let choice = offered_choice(&self.cli_readings, provider, model, effort, existing_effort)?;
        if provider == AgentProvider::ClaudeAdk && effort.is_some() && effort != existing_effort {
            return Err(format!(
                "set_agent_model: Claude ADK keeps its launch effort ({}). Omit effort or specify that effort.",
                existing_effort.unwrap_or("harness default")
            ));
        }
        if tab.session.turn_choice_support(&choice) != TurnChoiceSupport::Native {
            return Err(cannot_change());
        }

        let busy = match provider {
            AgentProvider::ClaudeAdk => {
                status == AgentStatus::Starting || tab.session.waits_for_turn_choice(&choice)
            }
            _ => status != AgentStatus::Waiting,
        };
        let deferred = busy || self.delivery_queue.agent_in_flight(&key);
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

fn offered_choice(
    readings: &Arc<Readings>,
    provider: AgentProvider,
    model: &str,
    requested_effort: Option<&str>,
    existing_effort: Option<&str>,
) -> Result<ModelChoice, String> {
    let offer = model_offer_from(readings, provider);
    let entry = offer
        .models
        .iter()
        .find(|entry| entry.id == model)
        .ok_or_else(|| {
            offer
                .refusal(model, harness_for(provider).cli_name())
                .unwrap_or_else(|| {
                    format!(
                        "set_agent_model: {} does not offer model {model:?}.",
                        provider.label()
                    )
                })
        })?;
    let effort = match requested_effort {
        None if provider == AgentProvider::ClaudeAdk && entry.efforts.is_empty() => None,
        _ => requested_effort.or(existing_effort),
    };
    if let Some(effort) = effort.filter(|effort| !entry.efforts.contains(effort)) {
        let effort = match requested_effort {
            Some(_) => format!("effort {effort}"),
            None => format!("the agent's current effort {effort}"),
        };
        return Err(format!(
            "set_agent_model: model {model} does not support {effort}."
        ));
    }
    let choice = ModelChoice {
        provider,
        model: Some(model.to_owned()),
        effort: effort.map(str::to_owned),
    };
    choice.validate()?;
    Ok(choice)
}
