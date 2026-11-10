use crate::app::{AppState, Project};
use crate::isolation::Isolation;
use crate::models::AgentProvider;
use serde_json::{json, Value};

/// What a volume that could not make the clone the settings asked for is said
/// with, spelled once so an operator reading the log and a human reading the
/// thread are told the same thing in the same words.
pub(in crate::app) fn isolation_downgrade_note(reason: &str) -> String {
    format!("Created a git worktree: Rift isolation is unavailable here — {reason}")
}

impl AppState {
    pub(in crate::app) fn config_value(
        &self,
        projects_dir: &std::path::Path,
        default_harness: AgentProvider,
        isolation: Isolation,
    ) -> Value {
        self.config_value_with_project(projects_dir, default_harness, isolation, None)
    }

    pub(in crate::app) fn config_value_with_project(
        &self,
        projects_dir: &std::path::Path,
        default_harness: AgentProvider,
        isolation: Isolation,
        prospective_project: Option<&Project>,
    ) -> Value {
        let mut config = json!({
            "next_environment": self.projects.next_environment() + u64::from(prospective_project.is_some_and(|p| self.projects.environment(&p.environment_id).is_none())),
            "environments": self.projects.environments().cloned().chain(prospective_project.filter(|p| self.projects.environment(&p.environment_id).is_none()).map(crate::app::projects::ProjectRegistry::environment_for_project)).map(|e| {
                json!({"id":e.id,"name":e.name,"next_source":e.next_source,"isolation":e.isolation,"sources":e.sources.iter().map(source_config).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "next_project": self.projects.next_id() + u64::from(prospective_project.is_some()),
            "projects_dir": projects_dir.display().to_string(),
            "default_harness": default_harness,
            "project_agent": self.project_agent,
            "role_models": self.role_models,
            "watch_agent_filed_tasks": self.watch_agent_filed_tasks,
            "compact_above_tokens": self.compact_above_tokens,
            // Absent until the device chooses: `null` leaves the default.
            "workspace_idle_secs": self.reclaim_settings.idle_after_secs,
            "workspace_prune": self.reclaim_settings.prune,
            "workspace_idle_digest_at": self.reclaim_settings.idle_digest_at.map(|at| at.to_string()),
            "agent_modes": self.agent_modes,
            "isolation": isolation,
            "router_model": self.router_choice,
            "retention": self.retention,
            "projects": self.projects.iter().chain(prospective_project).map(|p| {
                let mut entry = json!({
                    "id": p.id,
                    "name": p.name,
                    "environment_id": p.environment_id,
                    "project_key": p.project_key,
                    "home_path": p.home_path.display().to_string(),
                    "added_sources": p.added_sources.iter().map(source_config).collect::<Vec<_>>(),
                    "archived_at": p.archived_at,
                    "path": p.repo_path.display().to_string(),
                    "base_branch": p.base_branch,
                    // Past every source id this project has held, so a removed
                    // one is not reissued after a restart either.
                    "next_source": p.next_source,
                    "sources": p.sources.iter().map(source_config).collect::<Vec<_>>(),
                });
                if let Some(isolation) = p.isolation {
                    entry["isolation"] = json!(isolation);
                }
                entry
            }).chain(self.projects.parked().iter().cloned()).collect::<Vec<_>>(),
        });
        self.compaction_policies.write_config(&mut config);
        config
    }

    /// Save a fixture's already-installed configuration.
    #[cfg(test)]
    pub(in crate::app) fn persist(&self) {
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        if let Err(error) = self.persist_config(&config) {
            eprintln!("persist config: {error}");
        }
    }

    /// Persist projects and settings atomically, if persistence is configured.
    pub(in crate::app) fn persist_config(&self, config: &Value) -> Result<(), String> {
        self.mark_device_lifecycle_dirty()?;
        self.persist_secret_membership_config(self.config_path.as_deref(), config)
    }
}

fn source_config(source: &crate::app::projects::ProjectSource) -> Value {
    let mut value = json!({"id":source.id,"name":source.name,"mount":source.mount,
        "path":source.path.display().to_string(),"is_git":source.is_git,"base_branch":source.base_branch});
    if let Some(sync_base) = source.sync_base {
        value["sync_base"] = json!(sync_base);
    }
    value
}
