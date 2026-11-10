//! Delete registrations only; environment source folders belong to the user.
use crate::app::AppState;
use serde_json::{json, Value};

impl AppState {
    pub(crate) fn environment_delete(&mut self, environment_id: &str) -> Result<Value, String> {
        if self.projects.environment(environment_id).is_none() {
            return Ok(json!({}));
        }
        let count = self
            .projects
            .iter()
            .filter(|p| p.environment_id == environment_id)
            .count()
            + self
                .projects
                .parked()
                .iter()
                .filter(|p| p["environment_id"] == environment_id)
                .count()
            + self
                .project_deletion_fences
                .iter()
                .filter(|p| {
                    p.project_config["environment_id"] == environment_id
                        && self.projects.get(&p.project_id).is_none()
                        && !self
                            .projects
                            .parked()
                            .iter()
                            .any(|entry| entry["id"] == p.project_id)
                })
                .count();
        if count != 0 {
            return Err(format!("Delete its {count} projects first"));
        }
        self.refuse_pending_environment_creation(environment_id)?;
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["environments"]
            .as_array_mut()
            .expect("environment config array")
            .retain(|entry| entry["id"] != environment_id);
        // Denial must be durable before removing the external registration.
        self.revoke_environment_secrets_before_delete(environment_id)?;
        self.persist_config(&config)?;
        self.projects.remove_unused_environment(environment_id);
        self.note_board_lists_changed(crate::changes::BoardLists::ENVIRONMENTS);
        Ok(json!({}))
    }

    /// Commit grant retirement before removing the external registration.
    /// The writer owns its FULL transaction and notifies only after commit.
    fn revoke_environment_secrets_before_delete(&self, environment_id: &str) -> Result<(), String> {
        #[cfg(test)]
        if let Some(error) = &self.environment_secret_revocation_error {
            return Err(error.clone());
        }
        if let Some(store) = &self.store {
            store
                .writer()
                .secrets_retire_environment(environment_id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
