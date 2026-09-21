use super::{ProjectAgentPatch, SettingsPatch};
use crate::app::AppState;
use crate::harness::harness_for;
use crate::models::{self, AgentProvider, ProjectAgentChoice};
use serde_json::{json, Value};

impl AppState {
    /// Every account setting this bridge holds. `agent_modes` is independent;
    /// legacy mode aliases keep describing the concrete default harness for
    /// clients that still use those fields to choose that fallback.
    /// `project_agent` is what a project's agent starts on, beside the harness
    /// every other agent falls back to.
    pub(crate) fn settings_get(&self) -> Value {
        json!({
            "projects_dir": self.projects_dir.display().to_string(),
            "default_harness": self.default_harness,
            "project_agent": self.project_agent,
            "role_models": self.role_models,
            "watch_agent_filed_issues": self.watch_agent_filed_issues,
            "agent_modes": self.agent_modes,
            "claude_mode": models::claude_mode_of_harness(self.default_harness),
            "codex_mode": models::codex_mode_of_harness(self.default_harness),
            "isolation": self.isolation,
            "isolation_available": self.account_availability(),
        })
    }

    /// The catalog `models.list` answers with. What a start leads with is the
    /// account's answer, so the default provider is the account's default
    /// harness; `models`/`efforts` are that harness's catalog, repeated at the
    /// top level for clients that predate `providers`.
    pub(crate) fn models_list(&self) -> Value {
        json!({
            "models": harness_for(self.default_harness).models(),
            "efforts": harness_for(self.default_harness).effort_levels(),
            "default_provider": self.default_harness,
            "agent_modes": self.agent_modes,
            "providers": models::provider_catalogs(),
            // What this device says its models are FOR. It rides the catalog
            // because every surface that offers a model already reads this,
            // and a second call for three lines would be a second call on
            // every rail that paints a new-agent picker.
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
        let by_role = crate::models::AgentRole::ALL
            .into_iter()
            .map(|role| {
                let filled = self.role_models.for_role(role, None).map(|entry| {
                    json!({
                        "provider": entry.provider.unwrap_or(self.default_harness),
                        "model": entry.model,
                        "capability": entry.capability,
                        "direction": entry.capability.describes(),
                    })
                });
                (role.wire_id().to_string(), json!(filled))
            })
            .collect::<serde_json::Map<_, _>>();
        json!({
            "harnesses": models::provider_catalogs(),
            "default_harness": self.default_harness,
            "roles": crate::models::AgentRole::ALL
                .map(|role| json!({ "id": role.wire_id(), "describes": role.describes() })),
            "capabilities": crate::models::AgentCapability::ALL.map(|capability| json!({
                "id": capability.wire_id(),
                "direction": capability.describes(),
            })),
            // What this device has declared, in the user's own order.
            "role_models": self.role_models,
            // And which model answers each role right now.
            "roles_in_effect": by_role,
        })
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
    /// Which settings those are is [`SettingsPatch`]'s table; putting an
    /// accepted one to the account is [`AppState::apply_settings`].
    pub(crate) fn settings_set(&mut self, params: &Value) -> Result<Value, String> {
        let patch = SettingsPatch::parse(params, &self.account_availability())?;
        let agent_modes = patch
            .agent_modes
            .as_ref()
            .map(|value| self.agent_modes.merge_wire(value))
            .transpose()?
            .unwrap_or(self.agent_modes);
        // Every accepted field is put to a prospective config and written
        // BEFORE any of it reaches the account, so a refused write leaves
        // nothing applied. `projects_dir` is the one that touches the disk —
        // the folder is made here or the set is refused.
        let projects_dir = match patch.projects_dir {
            Some(dir) => {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                std::fs::canonicalize(&dir)
                    .map_err(|error| format!("cannot resolve {}: {error}", dir.display()))?
            }
            None => self.projects_dir.clone(),
        };
        let default_harness = patch.default_harness.unwrap_or(self.default_harness);
        let isolation = patch.isolation.unwrap_or(self.isolation);
        let project_agent =
            self.accepted_project_agent(patch.project_agent.as_ref(), default_harness)?;
        let role_models = patch
            .role_models
            .clone()
            .unwrap_or_else(|| self.role_models.clone());
        let watch_agent_filed_issues = patch
            .watch_agent_filed_issues
            .unwrap_or(self.watch_agent_filed_issues);
        let mut config = self.config_value(&projects_dir, default_harness, isolation);
        config["agent_modes"] = json!(agent_modes);
        config["project_agent"] = json!(project_agent);
        config["role_models"] = json!(role_models);
        config["watch_agent_filed_issues"] = json!(watch_agent_filed_issues);
        self.persist_config(&config)?;
        self.projects_dir = projects_dir;
        self.default_harness = default_harness;
        self.agent_modes = agent_modes;
        self.isolation = isolation;
        self.project_agent = project_agent;
        self.role_models = role_models;
        self.watch_agent_filed_issues = watch_agent_filed_issues;
        Ok(self.settings_get())
    }
}
