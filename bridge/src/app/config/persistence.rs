#[cfg(test)]
use crate::app::ConfigPersistStep;
use crate::app::{AppState, Project};
use crate::isolation::Isolation;
use crate::models::AgentProvider;
use serde_json::{json, Value};
use std::io::Write as _;

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
        json!({
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
            "agent_modes": self.agent_modes,
            "isolation": isolation,
            "router_model": self.router_choice,
            "projects": self.projects.iter().chain(prospective_project).map(|p| {
                let mut entry = json!({
                    "id": p.id,
                    "path": p.repo_path.display().to_string(),
                    "base_branch": p.base_branch,
                    // Past every source id this project has held, so a removed
                    // one is not reissued after a restart either.
                    "next_source": p.next_source,
                    "sources": p.sources.iter().map(|source| json!({
                        "id": source.id,
                        "name": source.name,
                        "mount": source.mount,
                        "path": source.path.display().to_string(),
                        "is_git": source.is_git,
                        "base_branch": source.base_branch,
                        "remote": source.remote,
                    })).collect::<Vec<_>>(),
                });
                if let Some(isolation) = p.isolation {
                    entry["isolation"] = json!(isolation);
                }
                entry
            }).collect::<Vec<_>>(),
        })
    }

    /// Write the config as it stands, for a verb that has already put its
    /// change to the account. The settings setter builds a prospective value
    /// instead, so a refused write leaves nothing applied.
    pub(in crate::app) fn persist(&self) {
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        if let Err(error) = self.persist_config(&config) {
            eprintln!("persist config: {error}");
        }
    }

    /// Persist projects and settings atomically, if persistence is configured.
    pub(in crate::app) fn persist_config(&self, config: &Value) -> Result<(), String> {
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("write config {}: {error}", path.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|error| format!("serialize config {}: {error}", path.display()))?;
        let temporary = path.with_extension("tmp");
        match std::fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "prepare config {}: cannot remove {}: {error}",
                    path.display(),
                    temporary.display()
                ))
            }
        }
        let write_result = (|| -> Result<(), String> {
            #[cfg(test)]
            if self.config_persist_failure == Some(ConfigPersistStep::Write) {
                return Err(format!(
                    "write config {}: injected write failure",
                    path.display()
                ));
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| format!("write config {}: {error}", path.display()))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("write config {}: {error}", path.display()))?;
            #[cfg(test)]
            if self.config_persist_failure == Some(ConfigPersistStep::Rename) {
                return Err(format!(
                    "write config {}: injected rename failure",
                    path.display()
                ));
            }
            std::fs::rename(&temporary, path)
                .map_err(|error| format!("write config {}: {error}", path.display()))
        })();
        if let Err(write_error) = write_result {
            match std::fs::remove_file(&temporary) {
                Ok(()) => return Err(write_error),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(write_error)
                }
                Err(cleanup_error) => {
                    return Err(format!(
                        "{write_error}; cannot remove temporary config {}: {cleanup_error}",
                        temporary.display()
                    ))
                }
            }
        }
        Ok(())
    }
}
