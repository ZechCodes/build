//! The `models.changed` push: an agent CLI on this machine changed what it
//! runs, so the model catalog a client holds is out of date (#203).
//!
//! The probing itself is a service of its own (`harness::installed`); this
//! only follows its change count for each greeted session, the way
//! `bridge.update_status` follows the update service.

use std::sync::Arc;

use serde_json::json;

use super::AppState;
use crate::carrier::SessionSender;
use crate::changes::MODELS_CHANGED_EVENT;
use crate::harness::harness_for;
use crate::harness::installed::{model_offer_from, Readings};
use crate::models::{AgentProvider, AgentRole, ModelChoice, RoleModel};

impl AppState {
    /// Serve catalogs, and announce their changes, from `readings` rather
    /// than the process's own.
    pub fn with_cli_readings(mut self, readings: Arc<Readings>) -> Self {
        self.cli_readings = readings;
        self
    }

    /// Tell `sender` each time a CLI's answer changes from here on. What it
    /// already holds is the catalog it asked for, so the answer standing now
    /// is not news.
    pub(in crate::app) fn subscribe_models_changed(&mut self, sender: &SessionSender) {
        // A greeting answered off any runtime (a synchronous test) has no one
        // to watch for it; a real session is always greeted on one.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.unsubscribe_models_changed(sender.session_id());
        let mut changes = self.cli_readings.changes();
        changes.mark_unchanged();
        let sender = sender.clone();
        let session_id = sender.session_id().to_string();
        let task = runtime.spawn(async move {
            while changes.changed().await.is_ok() {
                if !sender.push(json!({ "type": MODELS_CHANGED_EVENT })) {
                    break;
                }
            }
        });
        self.models_subscriptions.insert(session_id, task);
    }

    pub(in crate::app) fn unsubscribe_models_changed(&mut self, session_id: &str) {
        if let Some(task) = self.models_subscriptions.remove(session_id) {
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
        self.role_models.for_role_where(role, capability, |entry| {
            self.runs_here(entry.provider.unwrap_or(self.default_harness), &entry.model)
        })
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
