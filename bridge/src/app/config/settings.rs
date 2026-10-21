use super::{ProjectAgentPatch, SettingsPatch};
use crate::app::AppState;
use crate::harness::harness_for;
use crate::models::{self, AgentProvider, ProjectAgentChoice};
use serde_json::{json, Value};

mod values;
use values::SettingsValues;

impl AppState {
    /// Every account setting this bridge holds. `agent_modes` is independent;
    /// legacy mode aliases keep describing the concrete default harness for
    /// clients that still use those fields to choose that fallback.
    /// `project_agent` is what a project's agent starts on, beside the harness
    /// every other agent falls back to.
    pub(crate) fn settings_get(&self) -> Value {
        self.settings_get_observed(serde_json::json!(self.account_availability()))
    }

    pub(in crate::app) fn settings_get_observed(&self, availability: Value) -> Value {
        json!({
            "projects_dir": self.projects_dir.display().to_string(),
            "default_harness": self.default_harness,
            "project_agent": self.project_agent,
            "role_models": self.role_models,
            "watch_agent_filed_tasks": self.watch_agent_filed_tasks,
            "compact_above_tokens": self.compact_above_tokens,
            "compaction_thresholds": self.compaction_policies.device.read_value(),
            "compaction": self.scope_compaction_observation(&crate::compaction::CompactionScope::Device),
            "agent_modes": self.agent_modes,
            "claude_mode": models::claude_mode_of_harness(self.default_harness),
            "codex_mode": models::codex_mode_of_harness(self.default_harness),
            "isolation": self.isolation,
            "isolation_available": availability,
            // What the reclaim service sweeps by now (#167), and which of the
            // two an environment variable holds, so a client can say why its
            // control does not move them.
            "workspace_idle_secs": self.reclaim_policy_now().idle_after.as_secs(),
            "workspace_prune": self.reclaim_policy_now().prune,
            "workspace_pinned": self.reclaim_policy.pinned(),
        })
    }

    /// The catalog `models.list` answers with. What a start leads with is the
    /// account's answer, so the default provider is the account's default
    /// harness; `models`/`efforts` are that harness's catalog, repeated at the
    /// top level for clients that predate `providers`.
    pub(crate) fn models_list(&self) -> Value {
        json!({
            "models": crate::harness::installed::model_offer_from(&self.cli_readings, self.default_harness).models,
            "efforts": harness_for(self.default_harness).effort_levels(),
            "default_provider": self.default_harness,
            "agent_modes": self.agent_modes,
            "providers": models::provider_catalogs_from(&self.cli_readings),
            // What this device says its models are FOR. It rides the catalog
            // because every surface that offers a model already reads this,
            // and a second call for three lines would be a second call on
            // every rail that paints a new-agent picker.
            "role_models": self.role_models,
        })
    }

    /// Snapshot producers use the catalog service's completed observation;
    /// rendering canonical config cannot schedule a new CLI probe.
    pub(in crate::app) fn models_list_snapshot(&self) -> Value {
        let catalogs = self.cli_readings.catalog_snapshot();
        let selected = catalogs
            .iter()
            .find(|row| row.id == self.default_harness)
            .expect("the installed catalog includes every provider");
        json!({
            "models": selected.models,
            "efforts": selected.efforts,
            "default_provider": self.default_harness,
            "agent_modes": self.agent_modes,
            "providers": *catalogs,
            "role_models": self.role_models,
        })
    }

    /// What `list_harnesses` answers: every harness this bridge can run, what
    /// it can be asked for, whether it is here, and what this device has
    /// declared each model to be FOR.
    ///
    /// The roles are answered resolved — which model fills each one, and how
    /// much direction it wants — so an agent choosing a reviewer reads the
    /// answer rather than the rule.
    pub(in crate::app) fn harness_table(&self) -> Value {
        crate::app::model_catalog::harness_table_from(
            self.default_harness,
            &self.role_models,
            &self.cli_readings,
        )
    }

    /// What this set leaves the device holding for its project agents, refused
    /// here if the harness it names cannot be asked for that model or effort —
    /// before anything is written, like every other field of a set.
    fn accepted_project_agent(
        &self,
        asked: Option<&ProjectAgentPatch>,
        default_harness: AgentProvider,
    ) -> Result<ProjectAgentChoice, String> {
        let chosen = match asked {
            Some(asked) => asked.over(&self.project_agent),
            None => self.project_agent.clone(),
        };
        chosen.resolved(default_harness).validate()?;
        Ok(chosen)
    }

    /// Set the account settings a client names, and only those: where cloned
    /// repos land (creating the folder), which harness a new agent opens on,
    /// and how a new checkout is isolated.
    ///
    /// Which settings those are is [`SettingsPatch`]'s table; accepted values
    /// are staged together by [`SettingsValues`].
    pub(crate) fn settings_set(&mut self, params: &Value) -> Result<Value, String> {
        let availability = self.account_availability();
        let patch = SettingsPatch::parse(params, &availability)?;
        let mut prospective = SettingsValues::accepted(self, &patch)?;
        let compaction_changed = prospective.compaction_changed(self);
        if params.as_object().is_some_and(|params| params.len() == 1)
            && patch.compaction_thresholds.is_some()
            && !compaction_changed
        {
            return Ok(self.settings_get_observed(json!(availability)));
        }
        let config = prospective.config(self);
        let reclaim_changed = prospective.reclaim_changed(self);
        // The app lock keeps this temporary candidate private. Restore the
        // committed fields before persistence, retaining the fully owned delta
        // and response for an infallible completion after the config commits.
        prospective.swap(self);
        let projection = self.prepare_all_snapshot_projection();
        let response = self.settings_get_observed(json!(availability));
        prospective.swap(self);
        self.persist_config(&config)?;
        prospective.swap(self);
        self.publish_read_cold_harness_settings();
        if let Some(projection) = projection {
            self.publish_prepared_snapshot_projection(projection);
        }
        if reclaim_changed {
            self.nudge_workspace_reclaim();
        }
        if compaction_changed {
            self.note_compaction_changed("device", None);
        }
        Ok(response)
    }
}
