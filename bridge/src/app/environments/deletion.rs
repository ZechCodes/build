//! Delete registrations only; environment source folders belong to the user.
use crate::app::AppState;
use serde_json::{json, Value};

impl AppState {
    pub(crate) fn environment_delete(&mut self, environment_id: &str) -> Result<Value, String> {
        self.projects
            .environment(environment_id)
            .ok_or_else(|| format!("unknown environment: {environment_id}"))?;
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

    /// S2.1 integration seam. Schema 26 has no environment-scoped secrets.
    /// Replace this body with S2.1's durable revocation when the slices join;
    /// propagate failure so the environment registration is never removed first.
    fn revoke_environment_secrets_before_delete(
        &self,
        _environment_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}
