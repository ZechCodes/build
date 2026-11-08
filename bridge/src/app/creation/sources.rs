//! Source reads and clones precede config publication inside the same attempt.
use super::execution::{checkpoint_effect, progress, set_step};
use super::*;
use crate::app::projects::ProjectSource;
use crate::lifecycle::{CloneRepo, OpenRepo, WorktreeMutation};
use std::path::{Path, PathBuf};

pub(super) fn reserve(
    state: &Arc<Mutex<AppState>>,
    method: &str,
    params: &Value,
    operation: &str,
) -> Result<Value, ApiError> {
    let app = state.lock().unwrap();
    let parent = params["parent"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(crate::app::expand_tilde)
        .unwrap_or_else(|| app.projects_dir.clone());
    let root =
        crate::worktree::canonical_planned_path(&parent.join(format!("environment-{operation}")));
    app.validate_secret_source(&root).map_err(ApiError::from)?;
    let mut evidence = json!({"project_key":format!("project:{}",uuid::Uuid::new_v4()),"steps":[],"source_root":root,"source_owner":operation});
    let (inherited, project) = if method == "environment.create" {
        evidence["environment_id"] = json!(format!("env-{operation}"));
        let plan = app
            .plan_creation_sources(params["sources"].as_array().unwrap(), &[], &root)
            .map_err(ApiError::from)?;
        let inherited = planned_sources(&plan).map_err(ApiError::from)?;
        evidence["environment_source_count"] = json!(inherited.len());
        evidence["source_plan"] = plan;
        (inherited, &params["project"])
    } else {
        evidence["environment_id"] = params["environment_id"].clone();
        evidence["source_plan"] = json!([]);
        (
            app.projects
                .environment(params["environment_id"].as_str().unwrap())
                .unwrap()
                .sources
                .clone(),
            params,
        )
    };
    let extra = project["added_sources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let additions = app
        .plan_creation_sources(&extra, &inherited, &root)
        .map_err(ApiError::from)?;
    evidence["source_plan"]
        .as_array_mut()
        .unwrap()
        .extend(additions.as_array().unwrap().iter().cloned());
    // Planning never creates the parent. A failed attempt removes only ancestors
    // that were absent at admission, and only while they are still empty.
    evidence["source_parents"] = json!(missing_parents(&root));
    Ok(evidence)
}
fn planned_sources(plan: &Value) -> Result<Vec<ProjectSource>, String> {
    plan.as_array()
        .ok_or("source plan missing")?
        .iter()
        .map(|entry| serde_json::from_value(entry["source"].clone()).map_err(|e| e.to_string()))
        .collect()
}
fn missing_parents(root: &Path) -> Vec<PathBuf> {
    root.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|path| !path.exists())
        .map(Path::to_path_buf)
        .collect()
}
pub(super) fn prepare(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
    evidence: &mut Value,
) -> Result<(), String> {
    if evidence["sources_prepared"] == true
        || evidence["source_plan"].as_array().is_none_or(Vec::is_empty)
    {
        return Ok(());
    }
    let recovering_started =
        evidence["effect_step"] == "sources" && evidence["effect_state"] == "started";
    state.lock().unwrap().hold_creation_sources(evidence)?;
    progress(
        state,
        &record.operation_id,
        evidence,
        "sources",
        "running",
        None,
    );
    evidence["effect_step"] = json!("sources");
    checkpoint_effect(writer, &record.operation_id, evidence, "started")?;
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::SourceStarted);
    let prepared = prepare_all(evidence, recovering_started);
    let sources = match prepared {
        Ok(sources) => sources,
        Err(error) => {
            // A cleanup failure must never turn a known source failure into a
            // later provisioning run. Freeze intent before attempting unwind.
            evidence["source_failure"] = json!(error);
            evidence["failure_intent"] = json!({"error":error});
            writer
                .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
                .map_err(|e| e.to_string())?;
            rollback(state, writer, record, evidence, &error)?;
            return Err(error);
        }
    };
    evidence["prepared_sources"] = json!(sources);
    evidence["sources_prepared"] = json!(true);
    set_step(evidence, "sources", "done", None);
    evidence["effect_step"] = json!(if record.method == "environment.create" {
        "environment"
    } else {
        "project"
    });
    evidence["effect_state"] = json!("not_started");
    writer
        .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
        .map_err(|e| e.to_string())?;
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::SourcePrepared);
    progress(
        state,
        &record.operation_id,
        evidence,
        "sources",
        "done",
        None,
    );
    Ok(())
}
fn prepare_all(evidence: &Value, recovering_started: bool) -> Result<Vec<ProjectSource>, String> {
    let root = PathBuf::from(
        evidence["source_root"]
            .as_str()
            .ok_or("source root missing")?,
    );
    let plan = evidence["source_plan"]
        .as_array()
        .ok_or("source plan missing")?;
    if plan.iter().any(|entry| entry["remote"].is_string()) {
        if recovering_started && !clones_have_completion_proof(&root, plan) {
            return Err(
                "source clone was interrupted before completion was recorded; outcome unknown"
                    .into(),
            );
        }
        prepare_root(&root, evidence)?;
    }
    plan.iter().map(|entry| open_source(entry, &root)).collect()
}
fn prepare_root(root: &Path, evidence: &Value) -> Result<(), String> {
    if crate::worktree::canonical_planned_path(root) != root {
        return Err("source parent ownership proof changed; outcome unknown".into());
    }
    if std::fs::symlink_metadata(root).is_ok() {
        return prove_root(root, evidence);
    }
    let parent = root.parent().ok_or("source parent missing")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    std::fs::create_dir(root).map_err(|e| e.to_string())?;
    let marker = root.join(".build-creation-owner");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .map_err(|e| e.to_string())?;
    file.write_all(
        evidence["source_owner"]
            .as_str()
            .ok_or("source owner missing")?
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn prove_root(root: &Path, evidence: &Value) -> Result<(), String> {
    let marker = root.join(".build-creation-owner");
    let actual = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let owned = actual == root
        && std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file())
        && std::fs::read_to_string(marker).ok().as_deref() == evidence["source_owner"].as_str();
    if !owned {
        return Err("source ownership proof missing; outcome unknown".into());
    }
    Ok(())
}
fn open_source(entry: &Value, root: &Path) -> Result<ProjectSource, String> {
    let mut source: ProjectSource =
        serde_json::from_value(entry["source"].clone()).map_err(|e| e.to_string())?;
    let requested_base = entry["base_branch"].as_str().map(str::to_owned);
    let opened = if let Some(remote) = entry["remote"].as_str() {
        if crate::worktree::canonical_planned_path(&source.path) != source.path {
            return Err("source ownership proof changed; outcome unknown".into());
        }
        let completion = clone_completion(root, &source.id);
        if source.path.exists() && !completion_is_ready(&completion) {
            return Err("source clone completion proof missing; outcome unknown".into());
        }
        let opened = CloneRepo {
            url: remote.into(),
            name: source.name.clone(),
            dest: source.path.clone(),
            projects_dir: root.into(),
            requested_base,
        }
        .perform()?
        .output;
        if !completion.exists() {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&completion)
                .map_err(|e| e.to_string())?;
            file.write_all(b"ready").map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            std::fs::File::open(root)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
        }
        opened
    } else {
        OpenRepo {
            path: source.path.clone(),
            requested_base,
        }
        .perform()?
        .output
    };
    source.path = opened.path;
    source.is_git = opened.is_git;
    source.base_branch = opened.base;
    Ok(source)
}
fn clone_completion(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".build-{id}.ready"))
}
fn completion_is_ready(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
        && std::fs::read(path).is_ok_and(|value| value == b"ready")
}
fn clones_have_completion_proof(root: &Path, plan: &[Value]) -> bool {
    plan.iter()
        .filter(|entry| entry["remote"].is_string())
        .all(|entry| {
            entry["source"]["id"]
                .as_str()
                .is_some_and(|id| completion_is_ready(&clone_completion(root, id)))
                && entry["source"]["path"]
                    .as_str()
                    .is_some_and(|path| Path::new(path).exists())
        })
}
pub(super) fn prepared(evidence: &Value) -> Result<Vec<ProjectSource>, String> {
    if evidence["prepared_sources"].is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(evidence["prepared_sources"].clone()).map_err(|e| e.to_string())
}
/// The full source rollback is the only source path allowed to write settled.
/// Failure leaves Started intact, with its reservation and terminal fence held.
pub(super) fn rollback(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
    evidence: &mut Value,
    error: &str,
) -> Result<(), String> {
    let root = PathBuf::from(
        evidence["source_root"]
            .as_str()
            .ok_or("source root missing")?,
    );
    progress(
        state,
        &record.operation_id,
        evidence,
        "sources",
        "running",
        Some("Cleaning up…"),
    );
    if has_clones(evidence) {
        exclude_new_source_owners(state, evidence, &root)?;
        remove_owned_source_root(evidence, &root)?;
    }
    evidence["effect_state"] = json!("settled");
    evidence["failure_intent"] = json!({"error":error});
    writer
        .park_creation(
            PRINCIPAL,
            &record.operation_id,
            "reconciling",
            evidence.clone(),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn exclude_new_source_owners(
    state: &Arc<Mutex<AppState>>,
    evidence: &Value,
    root: &Path,
) -> Result<(), String> {
    let app = state.lock().unwrap();
    if let Some(claims) = evidence["source_owner"]
        .as_str()
        .and_then(|o| app.creation_source_claims.get(o))
    {
        for claim in claims {
            claim.hold_writing();
        }
    }
    if app.source_directory_has_owner(root) {
        return Err("source acquired another owner; outcome unknown".into());
    }
    Ok(())
}
fn remove_owned_source_root(evidence: &Value, root: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(root).is_ok() {
        prove_root(root, evidence)?;
        std::fs::remove_dir_all(root)
            .map_err(|e| format!("could not fully roll back source: {e}; outcome unknown"))?;
    }
    for path in evidence["source_parents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        match std::fs::remove_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "could not fully roll back source parent: {e}; outcome unknown"
                ))
            }
        }
    }
    Ok(())
}
fn has_clones(evidence: &Value) -> bool {
    evidence["source_plan"]
        .as_array()
        .is_some_and(|p| p.iter().any(|entry| entry["remote"].is_string()))
}
impl AppState {
    pub(in crate::app) fn hold_creation_sources(&mut self, evidence: &Value) -> Result<(), String> {
        let Some(owner) = evidence["source_owner"].as_str() else {
            return Ok(());
        };
        if self.creation_source_claims.contains_key(owner) {
            return Ok(());
        }
        let paths = evidence["source_plan"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["remote"].is_string())
            .filter_map(|e| e["source"]["path"].as_str().map(PathBuf::from))
            .collect::<Vec<_>>();
        let claims = self.source_directory_claims.reserve_destinations(&paths)?;
        self.creation_source_claims.insert(owner.into(), claims);
        Ok(())
    }
    pub(in crate::app) fn publish_creation_sources(&self, evidence: &Value) {
        if let Some(claims) = evidence["source_owner"]
            .as_str()
            .and_then(|o| self.creation_source_claims.get(o))
        {
            for claim in claims {
                claim.mark_prepared();
            }
        }
    }
}
