use super::*;
use crate::app::workspaces::journal_creation::{WorkspaceCreationEvent, WorkspaceCreationObserver};
use crate::workspace::Workspace;

pub(super) fn execute(
    state: &Arc<Mutex<AppState>>,
    mut record: MutationRecord,
) -> Result<(), String> {
    if matches!(record.state.as_str(), "succeeded" | "failed") {
        return Ok(());
    }
    let writer = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .ok_or("creation store missing")?
        .writer();
    if record.state == "accepted"
        && !writer
            .claim_mutation(PRINCIPAL, &record.operation_id)
            .map_err(string)?
    {
        return Ok(());
    }
    record = writer
        .creation_record(PRINCIPAL, &record.operation_id)
        .map_err(string)?
        .ok_or("creation disappeared")?;
    state
        .lock()
        .unwrap()
        .track_creation_workspace(&record.execution);
    let _activity = CreationActivity::new(state);
    let service = state.lock().unwrap().mutation_service();
    let result = if record.state == "reconciling" {
        cleanup(state, &writer, &record)
    } else {
        provision(state, &writer, &record)
    };
    match result {
        Ok(Some(result)) => {
            service.clear_progress(&record.operation_id);
            writer
                .finish_creation(
                    PRINCIPAL,
                    &record.operation_id,
                    json!({"state":"succeeded","result":result}),
                )
                .map_err(string)?;
        }
        Ok(None) => {}
        Err(error) => {
            finish_failure(state, &writer, &record.operation_id, &error)?;
        }
    }
    if let Some(record) = writer
        .creation_record(PRINCIPAL, &record.operation_id)
        .map_err(string)?
    {
        if matches!(record.state.as_str(), "succeeded" | "failed") {
            state
                .lock()
                .unwrap()
                .release_creation_workspace(&record.execution);
        }
    }
    Ok(())
}
/// Keep deletion and reclaim fenced through publication and terminal recording.
struct CreationActivity {
    state: std::sync::Weak<Mutex<AppState>>,
}
impl CreationActivity {
    fn new(state: &Arc<Mutex<AppState>>) -> Self {
        state.lock().unwrap().active_deferred_filesystem_jobs += 1;
        Self {
            state: Arc::downgrade(state),
        }
    }
}
impl Drop for CreationActivity {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            state.lock().unwrap().active_deferred_filesystem_jobs -= 1;
        }
    }
}
fn provision(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
) -> Result<Option<Value>, String> {
    let mut evidence = project_step(state, record)?;
    let id = evidence["project_id"]
        .as_str()
        .ok_or("missing saved project identity")?
        .to_string();
    let workspace_params = if record.method == "environment.create_project" {
        &record.params["workspace"]
    } else {
        &record.params
    };
    if evidence.get("workspace").is_none() {
        let reserved = state
            .lock()
            .unwrap()
            .reserve_creation_workspace(&id, workspace_params)?;
        evidence["workspace"] = json!(reserved);
        writer
            .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
            .map_err(string)?;
    }
    state.lock().unwrap().track_creation_workspace(&evidence);
    let reserved: Workspace =
        serde_json::from_value(evidence["workspace"].clone()).map_err(string)?;
    let recovered = {
        state
            .lock()
            .unwrap()
            .workspaces
            .recovered_creation_workspace(&reserved)?
    };
    if let Some(existing) = recovered {
        evidence["workspace"] = json!(existing);
        complete_workspace(state, writer, record, &mut evidence)?;
        return creation_result(
            state,
            record,
            &id,
            crate::app::workspaces::workspace_json(&existing),
        )
        .map(Some);
    }
    let evidence = Arc::new(Mutex::new(evidence));
    let observer = observer(state, writer, record, evidence.clone());
    progress(
        state,
        &record.operation_id,
        &evidence.lock().unwrap(),
        "workspace",
        "running",
        None,
    );
    let result = run_workspace(state, &reserved, Some(observer))?;
    let mut saved = evidence.lock().unwrap();
    complete_workspace(state, writer, record, &mut saved)?;
    drop(saved);
    creation_result(state, record, &id, result).map(Some)
}
fn complete_workspace(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
    evidence: &mut Value,
) -> Result<(), String> {
    evidence["workspace"]["status"] = json!("ready");
    set_step(evidence, "workspace", "done", None);
    writer
        .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
        .map_err(string)?;
    progress(
        state,
        &record.operation_id,
        evidence,
        "workspace",
        "done",
        None,
    );
    Ok(())
}
fn creation_result(
    state: &Arc<Mutex<AppState>>,
    record: &MutationRecord,
    id: &str,
    result: Value,
) -> Result<Value, String> {
    let app = state.lock().unwrap();
    let project = app.project_for(id)?;
    let result = if record.method == "environment.create_project" {
        json!({"project":app.project_json(project),"workspace":result})
    } else {
        result
    };
    crate::api::v1::check_read_result(&record.method, result.clone())
        .map_err(|e| e.message().to_string())?;
    Ok(result)
}
pub(super) fn project_step(
    state: &Arc<Mutex<AppState>>,
    record: &MutationRecord,
) -> Result<Value, String> {
    let writer = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .ok_or("creation store missing")?
        .writer();
    let mut evidence = record.execution.clone();
    if record.method != "environment.create_project" {
        return Ok(evidence);
    }
    progress(
        state,
        &record.operation_id,
        &evidence,
        "project",
        "running",
        None,
    );
    if evidence.get("project_id").is_none() {
        evidence["project_id"] =
            json!(format!("proj-{}", state.lock().unwrap().projects.next_id()));
        writer
            .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
            .map_err(string)?;
    }
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::ProjectCheckpointed);
    let key = evidence["project_key"]
        .as_str()
        .ok_or("key missing")?
        .to_owned();
    let provisional_id = evidence["project_id"]
        .as_str()
        .ok_or("id missing")?
        .to_owned();
    state.lock().unwrap().create_environment_project(
        record.params["environment_id"]
            .as_str()
            .ok_or("environment missing")?,
        record.params["name"].as_str().ok_or("name missing")?,
        &key,
        &provisional_id,
        |project_id| {
            evidence["project_id"] = json!(project_id);
            writer
                .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
                .map_err(string)?;
            #[cfg(test)]
            tests::at_boundary(tests::Boundary::ProjectIdentityCheckpointed);
            Ok(())
        },
    )?;
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::ProjectPersisted);
    set_step(&mut evidence, "project", "done", None);
    writer
        .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence.clone())
        .map_err(string)?;
    progress(
        state,
        &record.operation_id,
        &evidence,
        "project",
        "done",
        None,
    );
    Ok(evidence)
}
fn observer(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
    evidence: Arc<Mutex<Value>>,
) -> WorkspaceCreationObserver {
    let state = Arc::downgrade(state);
    let writer = writer.clone();
    let id = record.operation_id.clone();
    Arc::new(move |event| {
        let mut saved = evidence.lock().unwrap();
        match event {
            WorkspaceCreationEvent::Step {
                step,
                state: status,
                detail,
            } => {
                if let Some(app) = state.upgrade() {
                    progress(&app, &id, &saved, &step, &status, detail.as_deref());
                }
                if status != "running" {
                    set_step(&mut saved, &step, &status, detail.as_deref());
                    if status == "failed" {
                        writer
                            .checkpoint_creation(PRINCIPAL, &id, saved.clone())
                            .map_err(string)?;
                    }
                }
            }
            WorkspaceCreationEvent::Checkpoint(workspace) => {
                saved["workspace"] = json!(workspace);
                writer
                    .checkpoint_creation(PRINCIPAL, &id, saved.clone())
                    .map_err(string)?;
            }
            WorkspaceCreationEvent::Residue { workspace, error } => {
                saved["workspace"] = json!(workspace);
                saved["residue"] = json!({"paths":[workspace.root],"error":error});
                writer
                    .park_creation(PRINCIPAL, &id, "reconciling", saved.clone())
                    .map_err(string)?;
            }
        }
        Ok(())
    })
}
fn run_workspace(
    state: &Arc<Mutex<AppState>>,
    reserved: &Workspace,
    observer: Option<WorkspaceCreationObserver>,
) -> Result<Value, String> {
    let work = {
        let mut app = state.lock().unwrap();
        let work = app.prepare_creation_workspace(reserved, observer)?;
        app.active_deferred_filesystem_jobs += 1;
        app.active_device_registry_jobs += 1;
        work
    };
    run_work(state, work)
}
fn run_work(state: &Arc<Mutex<AppState>>, work: crate::app::DeferredGit) -> Result<Value, String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work.run()));
    let mut app = state.lock().unwrap();
    app.active_deferred_filesystem_jobs -= 1;
    app.active_device_registry_jobs -= 1;
    let result = result.map_err(|_| "workspace worker panicked; outcome unknown".to_string())?;
    let result = result.and_then(|value| work.call.settle(&mut app, value));
    work.call.invalidate(&mut app);
    app.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
    result
}
fn finish_failure(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    id: &str,
    error: &str,
) -> Result<(), String> {
    let record = writer
        .creation_record(PRINCIPAL, id)
        .map_err(string)?
        .ok_or("creation disappeared")?;
    let mut evidence = record.execution;
    if record.state == "reconciling" {
        progress(
            state,
            id,
            &evidence,
            "workspace",
            "failed",
            Some(&format!("Needs cleanup: {error}")),
        );
        return Ok(());
    }
    if ambiguous_workspace(&evidence, error) {
        state.lock().unwrap().mutation_service().clear_progress(id);
        writer
            .park_creation(PRINCIPAL, id, "unknown", evidence)
            .map_err(string)?;
        return Ok(());
    }
    // A workspace command captures its existing project's key for authority;
    // only environment.create_project can have landed a new project step.
    let created_key = evidence["project_key"]
        .as_str()
        .filter(|_| record.method == "environment.create_project");
    let (landed, details) = failure_resources(state, created_key);
    if landed {
        let project_id = details["project"]["project_id"]
            .as_str()
            .ok_or("landed project identity missing")?;
        evidence["project_id"] = json!(project_id);
        set_step(&mut evidence, "project", "done", None);
    }
    let step = if record.method == "workspace.create" || landed {
        "workspace"
    } else {
        "project"
    };
    set_step(&mut evidence, step, "failed", Some(error));
    let failed = writer
        .checkpoint_creation(PRINCIPAL, id, evidence)
        .map_err(string)?;
    progress(state, id, &failed.execution, step, "failed", Some(error));
    state.lock().unwrap().mutation_service().clear_progress(id);
    #[cfg(test)]
    tests::at_boundary(tests::Boundary::BeforeFailureTerminal);
    writer.finish_creation(PRINCIPAL,id,json!({"state":"failed","effect":if landed{"partial"}else{"unapplied"},"error":{"code":"creation_failed","message":error,"retryable":false,"details":details}})).map_err(string)?;
    Ok(())
}
fn ambiguous_workspace(evidence: &Value, error: &str) -> bool {
    if error.contains("outcome unknown") {
        return true;
    }
    let unproven = error.contains("ownership proof")
        || error.contains("another owner")
        || error.contains("symlink");
    unproven
        && evidence["workspace"]["root"]
            .as_str()
            .is_some_and(|root| std::fs::symlink_metadata(root).is_ok())
}
fn failure_resources(state: &Arc<Mutex<AppState>>, key: Option<&str>) -> (bool, Value) {
    let app = state.lock().unwrap();
    let project = app
        .projects
        .iter()
        .find(|p| Some(p.project_key.as_str()) == key);
    if let Some(project) = project {
        return (
            true,
            json!({"project":app.project_json(project),"workspace":null}),
        );
    }
    let parked = app
        .projects
        .parked()
        .iter()
        .find(|p| p["project_key"].as_str() == key && key.is_some());
    match parked {
        Some(project) => (
            true,
            json!({"project":{"project_id":project["id"],"name":project["name"]},"workspace":null}),
        ),
        None => (false, json!({"workspace":null})),
    }
}
fn cleanup(
    state: &Arc<Mutex<AppState>>,
    writer: &StoreWriter,
    record: &MutationRecord,
) -> Result<Option<Value>, String> {
    let reserved: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).map_err(string)?;
    progress(
        state,
        &record.operation_id,
        &record.execution,
        "workspace",
        "running",
        Some("Cleaning up…"),
    );
    let work = {
        let mut app = state.lock().unwrap();
        let work = app.prepare_creation_cleanup(&reserved)?;
        app.active_deferred_filesystem_jobs += 1;
        app.active_device_registry_jobs += 1;
        work
    };
    if let Err(error) = run_work(state, work) {
        progress(
            state,
            &record.operation_id,
            &record.execution,
            "workspace",
            "failed",
            Some(&format!("Needs cleanup: {error}")),
        );
        return Ok(None);
    }
    let mut evidence = record.execution.clone();
    evidence
        .as_object_mut()
        .ok_or("creation evidence missing")?
        .remove("residue");
    writer
        .checkpoint_creation(PRINCIPAL, &record.operation_id, evidence)
        .map_err(string)?;
    Err(record.execution["residue"]["error"]
        .as_str()
        .unwrap_or("workspace creation failed; residue cleaned")
        .into())
}
fn set_step(evidence: &mut Value, step: &str, status: &str, detail: Option<&str>) {
    if !evidence["steps"].is_array() {
        evidence["steps"] = json!([]);
    }
    let steps = evidence["steps"].as_array_mut().unwrap();
    let value = json!({"step":step,"state":status,"detail":detail});
    if let Some(old) = steps.iter_mut().find(|old| old["step"] == step) {
        *old = value;
    } else {
        steps.push(value);
    }
    steps.sort_by_key(|item| match item["step"].as_str() {
        Some("project") => 0,
        Some("workspace") => 1,
        _ => 2,
    });
}
fn progress(
    state: &Arc<Mutex<AppState>>,
    id: &str,
    evidence: &Value,
    step: &str,
    status: &str,
    detail: Option<&str>,
) {
    let service = state.lock().unwrap().mutation_service();
    let mut list: Vec<WriteProgress> = service
        .current_progress(id)
        .unwrap_or_else(|| serde_json::from_value(evidence["steps"].clone()).unwrap_or_default());
    list.retain(|item| item.step != step);
    list.push(WriteProgress {
        step: step.into(),
        state: match status {
            "done" => ProgressState::Done,
            "failed" => ProgressState::Failed,
            _ => ProgressState::Running,
        },
        detail: detail.map(|d| json!(d)),
    });
    service.set_progress(id, list);
}
fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
