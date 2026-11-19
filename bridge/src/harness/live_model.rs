//! Admission for changing the model choice on an existing session.

use super::installed::{model_offer_from, OfferedModel, Readings};
use super::{AgentSession, AgentStatus, Harness, TurnChoiceSupport};
use crate::models::ModelChoice;
use std::sync::Arc;

pub struct LiveModelChange<'a> {
    pub readings: &'a Arc<Readings>,
    pub session: &'a dyn AgentSession,
    pub status: AgentStatus,
    pub model: &'a str,
    pub requested_effort: Option<&'a str>,
    pub saved_effort: Option<&'a str>,
    pub launch_effort: Option<&'a str>,
}

pub struct LiveModelChoice {
    pub choice: ModelChoice,
    pub deferred: bool,
}

pub(super) fn unsupported_model_change(label: &str) -> String {
    format!("set_agent_model: {label} cannot change model in place on this session.")
}

impl LiveModelChange<'_> {
    pub(super) fn offered_model(&self, harness: &dyn Harness) -> Result<OfferedModel, String> {
        if !self.session.accepts_turn_choice() {
            return Err(unsupported_model_change(harness.label()));
        }
        let offer = model_offer_from(self.readings, harness.provider());
        offer
            .models
            .iter()
            .find(|entry| entry.id == self.model)
            .cloned()
            .ok_or_else(|| {
                offer
                    .refusal(self.model, harness.cli_name())
                    .unwrap_or_else(|| {
                        format!(
                            "set_agent_model: {} does not offer model {:?}.",
                            harness.label(),
                            self.model
                        )
                    })
            })
    }

    pub(super) fn choice(
        &self,
        harness: &dyn Harness,
        model: &OfferedModel,
        effort: Option<&str>,
    ) -> Result<ModelChoice, String> {
        if let Some(effort) = effort.filter(|effort| !model.efforts.contains(effort)) {
            let effort = match self.requested_effort {
                Some(_) => format!("effort {effort}"),
                None => format!("the agent's current effort {effort}"),
            };
            return Err(format!(
                "set_agent_model: model {} does not support {effort}.",
                model.id
            ));
        }
        let choice = ModelChoice {
            provider: harness.provider(),
            model: Some(model.id.clone()),
            effort: effort.map(str::to_owned),
        };
        choice.validate()?;
        Ok(choice)
    }

    pub(super) fn require_native(
        &self,
        harness: &dyn Harness,
        choice: &ModelChoice,
    ) -> Result<(), String> {
        match self.session.turn_choice_support(choice) {
            TurnChoiceSupport::Native => Ok(()),
            TurnChoiceSupport::RestartRequired => Err(unsupported_model_change(harness.label())),
        }
    }
}
