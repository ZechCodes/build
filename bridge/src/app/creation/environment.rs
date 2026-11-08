//! Environment config is committed independently of its first project.
use super::execution::{checkpoint_effect, progress, set_step};
use super::*;
use crate::app::environments::Environment;

pub(super) fn step(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
    evidence: &mut Value,
) -> Result<(), String> {
    if record.method != "environment.create" {
        return Ok(());
    }
    let id = evidence["environment_id"]
        .as_str()
        .ok_or("environment identity missing")?
        .to_owned();
    let count = evidence["environment_source_count"]
        .as_u64()
        .ok_or("environment sources missing")? as usize;
    let mut sources = super::sources::prepared(evidence)?;
    sources.truncate(count);
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::EnvironmentCheckpointed);
    progress(
        state,
        &record.operation_id,
        evidence,
        "environment",
        "running",
        None,
    );
    // Replay of a later step must preserve that step's Started or settled proof.
    if evidence["effect_step"] == "environment" {
        checkpoint_effect(writer, &record.operation_id, evidence, "started")?;
    }
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::EnvironmentStarted);
    if let Err(error) = persist(state, record, evidence, &id, sources) {
        if evidence["effect_step"] == "environment"
            && state.lock().unwrap().projects.environment(&id).is_none()
        {
            // Atomic config replacement failed before rename; no environment
            // by this reserved identity landed. Preserve that positive proof.
            checkpoint_effect(writer, &record.operation_id, evidence, "not_started")?;
        }
        return Err(error);
    }
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::EnvironmentPersisted);
    set_step(evidence, "environment", "done", None);
    if evidence["effect_step"] == "environment" {
        evidence["effect_step"] = json!("project");
        evidence["effect_state"] = json!("not_started");
    }
    writer
        .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
        .map_err(|e| e.to_string())?;
    progress(
        state,
        &record.operation_id,
        evidence,
        "environment",
        "done",
        None,
    );
    Ok(())
}
fn persist(
    state: &Arc<Mutex<AppState>>,
    record: &MutationRecord,
    evidence: &Value,
    id: &str,
    mut sources: Vec<crate::app::projects::ProjectSource>,
) -> Result<(), String> {
    let mut app = state.lock().unwrap();
    if app.projects.environment(id).is_some() {
        return Ok(());
    }
    if evidence["effect_step"] != "environment" {
        return Err(
            "saved creation environment identity is no longer available; outcome unknown".into(),
        );
    }
    app.publish_creation_sources(evidence);
    app.validate_creation_environment_sources(&mut sources, &cloned_paths(evidence))?;
    let home = sources.first().ok_or("environment has no sources")?;
    let name = record.params["name"]
        .as_str()
        .unwrap_or(&home.name)
        .to_owned();
    let next_source = sources.len() as u64 + 1;
    app.persist_creation_environment(Environment {
        id: id.into(),
        name,
        sources,
        next_source,
        isolation: record.params["isolation"]
            .as_str()
            .and_then(crate::isolation::Isolation::from_wire),
    })
}
pub(super) fn cloned_paths(evidence: &Value) -> Vec<std::path::PathBuf> {
    evidence["source_plan"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["remote"].is_string())
        .filter_map(|e| e["source"]["path"].as_str().map(std::path::PathBuf::from))
        .collect()
}
impl AppState {
    pub(crate) fn environment_discard_unused(&mut self, params: &Value) -> Result<Value, String> {
        let id = crate::app::require_str(params, "environment_id")?;
        let operation = crate::app::require_str(params, "creation_operation_id")?;
        let record = self
            .store
            .as_ref()
            .ok_or("discard needs a durable store")?
            .writer()
            .creation_record(PRINCIPAL, &operation)
            .map_err(|e| e.to_string())?
            .ok_or("creation attempt not found")?;
        if record.method != "environment.create" || record.execution["environment_id"] != id {
            return Err("environment was not created by this attempt".into());
        }
        if !matches!(record.state.as_str(), "succeeded" | "failed") {
            return Err("creation is still checking or needs cleanup".into());
        }
        if self.projects.iter().any(|p| p.environment_id == id)
            || self
                .projects
                .parked()
                .iter()
                .any(|p| p["environment_id"] == id)
        {
            return Err("environment has projects".into());
        }
        let writer = self.store.as_ref().unwrap().writer();
        if writer
            .pending_creations()
            .map_err(|e| e.to_string())?
            .iter()
            .any(|r| r.execution["environment_id"] == id || r.scope["environment_id"] == id)
        {
            return Err("environment has a pending creation".into());
        }
        self.projects
            .environment(&id)
            .ok_or("unknown environment")?;
        let mut config =
            self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        config["environments"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["id"] != id);
        self.persist_config(&config)?;
        self.projects.remove_unused_environment(&id);
        self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        Ok(json!({}))
    }
}
