//! The `models.changed` push: an agent CLI on this machine changed what it
//! runs, so the model catalog a client holds is out of date (#203).
//!
//! The probing itself is a service of its own (`harness::installed`); this
//! only follows its change count for each greeted session, the way
//! `bridge.update_status` follows the update service.

use std::sync::Arc;

use super::AppState;
use crate::changes::models_changed_payload;
use crate::execution::CallContext as SessionSender;
use crate::harness::harness_for;
use crate::harness::installed::{model_offer_from, Readings};
use crate::models::{
    self, AgentCapability, AgentProvider, AgentRole, ModelChoice, RoleModel, RoleModels,
};
use serde_json::{json, Value};

impl AppState {
    /// Serve catalogs, and announce their changes, from `readings` rather
    /// than the process's own.
    pub fn with_cli_readings(mut self, readings: Arc<Readings>) -> Self {
        self.cli_readings = readings;
        if self.snapshot_projection_initialized() {
            self.cli_readings.warm();
        }
        self
    }

    /// Tell `sender` each time a CLI's answer changes from here on. What it
    /// already holds is the catalog it asked for, so the answer standing now
    /// is not news.
    pub(in crate::app) fn subscribe_models_changed(&mut self, sender: &SessionSender) {
        if !sender.events_are_live() {
            return;
        }
        // A greeting answered off any runtime (a synchronous test) has no one
        // to watch for it; a real session is always greeted on one.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.unsubscribe_models_changed(sender.session_id());
        let mut changes = self.cli_readings.changes();
        changes.mark_unchanged();
        let context = sender.event_context();
        let sender = context.clone();
        let session_id = sender.session_id().to_string();
        let task = runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                if !sender.push(models_changed_payload()) {
                    break;
                }
            }
        });
        self.models_subscriptions
            .insert(session_id, (context, task));
    }

    pub(in crate::app) fn unsubscribe_models_changed(&mut self, session_id: &str) {
        if let Some((_, task)) = self.models_subscriptions.remove(session_id) {
            task.abort();
        }
    }

    /// Whether this machine's CLI for `provider` runs `model`, as far as it
    /// has said.
    fn runs_here(&self, provider: AgentProvider, model: &str) -> bool {
        model_offer_from(&self.cli_readings, provider)
            .refusal(model, harness_for(provider).cli_name())
            .is_none()
    }

    /// The model the user declared first for `role`, of those this machine
    /// runs.
    pub(in crate::app) fn role_model_here(
        &self,
        role: AgentRole,
        capability: Option<crate::models::AgentCapability>,
    ) -> Option<&RoleModel> {
        role_model_from(
            self.default_harness,
            &self.role_models,
            &self.cli_readings,
            role,
            capability,
        )
    }

    /// `choice`, less a model this machine's CLI cannot run: a default the
    /// user set is started on the harness's own model rather than refused.
    pub(in crate::app) fn runnable_default(&self, mut choice: ModelChoice) -> ModelChoice {
        if choice
            .model
            .as_deref()
            .is_some_and(|model| !self.runs_here(choice.provider, model))
        {
            choice.model = None;
        }
        choice
    }
}

/// Resolve the existing catalogue from captured, committed settings. This may
/// inspect executable identity and schedule normal CLI refreshes, so independent
/// readers call it only after releasing AppState.
pub(in crate::app) fn harness_table_from(
    default_harness: AgentProvider,
    role_models: &RoleModels,
    readings: &Arc<Readings>,
) -> Value {
    let by_role = crate::models::AgentRole::ALL
        .into_iter()
        .map(|role| {
            let filled =
                role_model_from(default_harness, role_models, readings, role, None).map(|entry| {
                    json!({
                        "provider": entry.provider.unwrap_or(default_harness),
                        "model": entry.model,
                        "capability": entry.capability,
                        "direction": entry.capability.describes(),
                    })
                });
            (role.wire_id().to_string(), json!(filled))
        })
        .collect::<serde_json::Map<_, _>>();
    json!({
        "harnesses": models::provider_catalogs_from(readings),
        "default_harness": default_harness,
        "roles": crate::models::AgentRole::ALL
            .map(|role| json!({ "id": role.wire_id(), "describes": role.describes() })),
        "capabilities": crate::models::AgentCapability::ALL.map(|capability| json!({
            "id": capability.wire_id(),
            "direction": capability.describes(),
        })),
        // What this device has declared, in the user's own order.
        "role_models": role_models,
        // And which model answers each role right now.
        "roles_in_effect": by_role,
    })
}

fn role_model_from<'a>(
    default_harness: AgentProvider,
    role_models: &'a RoleModels,
    readings: &Arc<Readings>,
    role: AgentRole,
    capability: Option<AgentCapability>,
) -> Option<&'a RoleModel> {
    role_models.for_role_where(role, capability, |entry| {
        let provider = entry.provider.unwrap_or(default_harness);
        model_offer_from(readings, provider)
            .refusal(&entry.model, harness_for(provider).cli_name())
            .is_none()
    })
}
