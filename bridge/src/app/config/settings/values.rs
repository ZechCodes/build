use super::{AppState, SettingsPatch};
use crate::agent_modes::AgentModes;
use crate::app::config::CompactionPolicies;
use crate::isolation::Isolation;
use crate::models::{AgentProvider, ProjectAgentChoice, RoleModels};
use crate::reclaim::ReclaimSettings;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Prospective canonical config only. Swapping these values while the caller
/// owns AppState lets it prepare immutable responses without cloning sessions.
pub(super) struct SettingsValues {
    projects_dir: PathBuf,
    default_harness: AgentProvider,
    agent_modes: AgentModes,
    isolation: Isolation,
    project_agent: ProjectAgentChoice,
    role_models: RoleModels,
    watch_agent_filed_tasks: bool,
    compact_above_tokens: u64,
    compaction_policies: CompactionPolicies,
    reclaim_settings: ReclaimSettings,
}

impl SettingsValues {
    pub(super) fn accepted(app: &AppState, patch: &SettingsPatch) -> Result<Self, String> {
        let default_harness = patch.default_harness.unwrap_or(app.default_harness);
        let agent_modes = patch
            .agent_modes
            .as_ref()
            .map(|value| app.agent_modes.merge_wire(value))
            .transpose()?
            .unwrap_or(app.agent_modes);
        Ok(Self {
            projects_dir: projects_dir(app, patch)?,
            default_harness,
            agent_modes,
            isolation: patch.isolation.unwrap_or(app.isolation),
            project_agent: app
                .accepted_project_agent(patch.project_agent.as_ref(), default_harness)?,
            role_models: patch
                .role_models
                .clone()
                .unwrap_or_else(|| app.role_models.clone()),
            watch_agent_filed_tasks: patch
                .watch_agent_filed_tasks
                .unwrap_or(app.watch_agent_filed_tasks),
            compact_above_tokens: patch
                .compact_above_tokens
                .unwrap_or(app.compact_above_tokens),
            compaction_policies: compaction_policies(app, patch)?,
            reclaim_settings: ReclaimSettings {
                idle_after_secs: patch
                    .workspace_idle_secs
                    .or(app.reclaim_settings.idle_after_secs),
                prune: patch.workspace_prune.or(app.reclaim_settings.prune),
            },
        })
    }

    pub(super) fn compaction_changed(&self, app: &AppState) -> bool {
        self.compaction_policies.device != app.compaction_policies.device
            || self.compact_above_tokens != app.compact_above_tokens
    }

    pub(super) fn reclaim_changed(&self, app: &AppState) -> bool {
        self.reclaim_settings != app.reclaim_settings
    }

    pub(super) fn config(&self, app: &AppState) -> Value {
        let mut config = app.config_value(&self.projects_dir, self.default_harness, self.isolation);
        self.compaction_policies.write_config(&mut config);
        config["agent_modes"] = json!(self.agent_modes);
        config["project_agent"] = json!(self.project_agent);
        config["role_models"] = json!(self.role_models);
        config["watch_agent_filed_tasks"] = json!(self.watch_agent_filed_tasks);
        config["compact_above_tokens"] = json!(self.compact_above_tokens);
        config["workspace_idle_secs"] = json!(self.reclaim_settings.idle_after_secs);
        config["workspace_prune"] = json!(self.reclaim_settings.prune);
        config
    }

    pub(super) fn swap(&mut self, app: &mut AppState) {
        std::mem::swap(&mut self.projects_dir, &mut app.projects_dir);
        std::mem::swap(&mut self.default_harness, &mut app.default_harness);
        std::mem::swap(&mut self.agent_modes, &mut app.agent_modes);
        std::mem::swap(&mut self.isolation, &mut app.isolation);
        std::mem::swap(&mut self.project_agent, &mut app.project_agent);
        std::mem::swap(&mut self.role_models, &mut app.role_models);
        std::mem::swap(
            &mut self.watch_agent_filed_tasks,
            &mut app.watch_agent_filed_tasks,
        );
        std::mem::swap(
            &mut self.compact_above_tokens,
            &mut app.compact_above_tokens,
        );
        std::mem::swap(&mut self.compaction_policies, &mut app.compaction_policies);
        std::mem::swap(&mut self.reclaim_settings, &mut app.reclaim_settings);
    }
}

fn projects_dir(app: &AppState, patch: &SettingsPatch) -> Result<PathBuf, String> {
    let Some(dir) = &patch.projects_dir else {
        return Ok(app.projects_dir.clone());
    };
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    std::fs::canonicalize(dir).map_err(|error| format!("cannot resolve {}: {error}", dir.display()))
}

fn compaction_policies(
    app: &AppState,
    patch: &SettingsPatch,
) -> Result<CompactionPolicies, String> {
    let mut policies = app.compaction_policies.clone();
    if let Some(tokens) = patch.compact_above_tokens {
        policies.device = crate::compaction::CompactionThresholds::broadcast(tokens);
    }
    if let Some(map) = &patch.compaction_thresholds {
        policies.device = policies.device.patched(map)?;
    }
    Ok(policies)
}
