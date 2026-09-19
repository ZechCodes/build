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
        let mut config = self.config_value(&projects_dir, default_harness, isolation);
        config["agent_modes"] = json!(agent_modes);
        config["project_agent"] = json!(project_agent);
        self.persist_config(&config)?;
        self.projects_dir = projects_dir;
        self.default_harness = default_harness;
        self.agent_modes = agent_modes;
        self.isolation = isolation;
        self.project_agent = project_agent;
        Ok(self.settings_get())
    }
}
