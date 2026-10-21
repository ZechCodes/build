use super::{ReadView, ReadViewSlot};
use crate::api::{self, ApiError};
use crate::transport::Frame;
use serde_json::{json, Value};

mod streams;

pub(in crate::app) struct SnapshotService {
    view: ReadViewSlot,
    inventory: std::sync::Arc<crate::harness::inventory::Inventory>,
    readings: std::sync::Arc<crate::harness::installed::Readings>,
    updates: Option<std::sync::Arc<crate::update::UpdateService>>,
    admission: Option<std::sync::Arc<crate::update::AdmissionGate>>,
    qa_agent: bool,
}

impl SnapshotService {
    pub fn new(app: &super::super::AppState) -> Self {
        Self {
            view: app.read_view.clone(),
            inventory: std::sync::Arc::clone(app.harness_inventory()),
            readings: std::sync::Arc::clone(&app.cli_readings),
            updates: app.update_service(),
            admission: app.update_admission(),
            qa_agent: app.qa_agent,
        }
    }

    pub fn call(&self, frame: Frame) -> Value {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let method = frame.payload["method"].as_str().unwrap_or("");
        let params = frame
            .payload
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({}));
        api::reply(id, self.read(method, &params))
    }

    fn read(&self, method: &str, params: &Value) -> Result<Value, ApiError> {
        let _lease = self.native_admission(method)?;
        if matches!(method, "stream.events" | "stream.state") {
            return streams::read(&self.view.load(), method, params, self.qa_agent);
        }
        let params = crate::api::v1::normalize_read_params(method, params)?;
        if method == "harnesses.list" {
            return serialized(method, self.inventory.snapshot());
        }
        if method == "bridge.update_status" {
            let service = self
                .updates
                .as_ref()
                .ok_or_else(|| ApiError::unavailable("bridge updates are unavailable"))?;
            return serialized(method, service.status());
        }
        let view = self.view.load();
        if method == "tasks.columns"
            && view
                .observations
                .as_ref()
                .is_some_and(|context| context.deleting_project())
        {
            return Err(ApiError::unavailable(
                crate::app::rpc::PROJECT_DELETION_IN_PROGRESS,
            ));
        }
        let (method, owner) = read_key(method, &params)?;
        let mut answer = if method == "workspace.list" && params["project_id"].as_str() == Some("")
        {
            json!({"workspaces": []})
        } else {
            match view.get_owned(method, owner) {
                Some(value) => value,
                None => missing(method, owner)?,
            }
        };
        if matches!(method, "workspace.get" | "task.get") {
            answer = owner_detail(&view, &params, method, owner, answer)?;
        }
        if method == "agent.list" {
            filter_project_workers(&params, &mut answer);
        }
        if method == "models.list" {
            overlay_catalog(&self.readings, &mut answer)?;
        }
        crate::api::v1::check_read_result(method, answer)
    }

    fn native_admission(
        &self,
        method: &str,
    ) -> Result<Option<crate::update::AdmissionLease>, ApiError> {
        if !matches!(
            method,
            "capture.get"
                | "task.get"
                | "task.stages"
                | "tasks.columns"
                | "stream.events"
                | "stream.state"
        ) {
            return Ok(None);
        }
        self.admission
            .as_ref()
            .map(|gate| {
                gate.try_enter()
                    .ok_or_else(|| ApiError::busy("bridge update handoff in progress"))
            })
            .transpose()
    }
}

fn overlay_catalog(
    readings: &crate::harness::installed::Readings,
    answer: &mut Value,
) -> Result<(), ApiError> {
    let provider: crate::models::AgentProvider =
        serde_json::from_value(answer["default_provider"].clone())
            .map_err(|error| ApiError::internal(error.to_string()))?;
    let catalogs = readings.catalog_snapshot();
    let selected = catalogs
        .iter()
        .find(|row| row.id == provider)
        .ok_or_else(|| ApiError::internal("default provider is missing from catalog"))?;
    answer["models"] = json!(selected.models);
    answer["efforts"] = json!(selected.efforts);
    answer["providers"] = json!(*catalogs);
    Ok(())
}

fn read_key<'a>(method: &str, params: &'a Value) -> Result<(&'static str, &'a str), ApiError> {
    match method {
        "board.list" => Ok(("board.list", "")),
        "project.list" => Ok(("project.list", "")),
        "settings.get" => Ok(("settings.get", "")),
        "models.list" => Ok(("models.list", "")),
        "harnesses.list" => Ok(("harnesses.list", "")),
        "bridge.update_status" => Ok(("bridge.update_status", "")),
        "workspace.list" => Ok((
            "workspace.list",
            params["project_id"].as_str().unwrap_or(""),
        )),
        "workspace.get" => Ok((
            "workspace.get",
            params["workspace_id"].as_str().unwrap_or(""),
        )),
        "agent.list" => Ok(("agent.list", params["entity_id"].as_str().unwrap_or(""))),
        "capture.get" => Ok(("capture.get", params["capture_id"].as_str().unwrap_or(""))),
        "task.get" => Ok(("task.get", params["task_id"].as_str().unwrap_or(""))),
        "task.stages" => Ok(("task.stages", params["task_id"].as_str().unwrap_or(""))),
        "tasks.columns" => Ok(("tasks.columns", params["project_id"].as_str().unwrap_or(""))),
        _ => Err(ApiError::unknown_method(method)),
    }
}

fn missing(method: &str, owner: &str) -> Result<Value, ApiError> {
    match method {
        "workspace.list" => Ok(json!({"workspaces": []})),
        "capture.get" => Err(ApiError::classify(format!("unknown capture_id: {owner}"))),
        "task.get" => Err(ApiError::classify("unknown plan_id".into())),
        "task.stages" => Err(ApiError::classify("unknown task_id".into())),
        "tasks.columns" => Err(ApiError::classify(format!("unknown project_id: {owner}"))),
        "workspace.get" => Err(ApiError::classify(format!("unknown workspace_id: {owner}"))),
        "agent.list" => Err(crate::api::v1::thread::refine(ApiError::classify(format!(
            "agent.list: unknown entity {owner}"
        )))),
        "bridge.update_status" => Err(ApiError::unavailable("bridge updates are unavailable")),
        _ => Err(ApiError::internal(format!(
            "{method}: snapshot was not initialized"
        ))),
    }
}

pub(in crate::app) fn filter_project_workers(params: &Value, answer: &mut Value) {
    if params["include_project_workers"].as_bool() == Some(true) {
        return;
    }
    let Some(pin) = answer["pinned_agent_id"].as_str().map(str::to_owned) else {
        return;
    };
    if let Some(agents) = answer["agents"].as_array_mut() {
        agents.retain(|agent| agent["id"].as_str() == Some(&pin));
    }
}

fn serialized(method: &str, value: impl serde::Serialize) -> Result<Value, ApiError> {
    let value =
        serde_json::to_value(value).map_err(|error| ApiError::internal(error.to_string()))?;
    crate::api::v1::check_read_result(method, value)
}

impl super::super::AppState {
    pub(in crate::app) fn snapshot_service(&self) -> SnapshotService {
        SnapshotService::new(self)
    }

    /// The initialized in-process facade uses the same pure read implementation
    /// as carrier intake. Startup fixtures keep their pre-publication path.
    pub(crate) fn snapshot_read(
        &self,
        method: &str,
        params: &Value,
    ) -> Option<Result<Value, ApiError>> {
        (self.snapshot_projection_initialized()
            && crate::api::effects::INTAKE_SNAPSHOT_METHODS.contains(&method))
        .then(|| self.snapshot_service().read(method, params))
    }
}

fn owner_detail(
    view: &ReadView,
    params: &Value,
    method: &'static str,
    owner: &str,
    mut answer: Value,
) -> Result<Value, ApiError> {
    if answer["thread"].is_null() {
        return match params["agent_id"].as_str() {
            Some(agent) => Err(ApiError::classify(format!("unknown agent_id: {agent}"))),
            None => Ok(answer),
        };
    }
    let named_agent = optional_identity(params, "agent_id")?;
    if let Some(agent) = named_agent {
        let key = format!("{owner}/{agent}");
        answer = view.get_owned(method, &key).ok_or_else(|| {
            let error = view
                .get("task.get.error", &key)
                .as_deref()
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("unknown agent_id: {agent}"));
            ApiError::classify(error)
        })?;
    }
    let Some(canonical_agent) = answer["thread"]["agent"]["id"].as_str() else {
        return Ok(answer);
    };
    if canonical_agent.is_empty()
        && (params.get("conversation_id").is_some() || crate::app::thread_cursor(params).is_some())
    {
        return Err(ApiError::classify(crate::agent::NO_AGENT_YET.to_owned()));
    }
    let expected_conversation = optional_identity(params, "conversation_id")?;
    if let Some(expected) = expected_conversation {
        if expected != canonical_agent {
            let agent = named_agent
                .or_else(|| answer["agents"].as_array()?.first()?["id"].as_str())
                .unwrap_or(canonical_agent);
            return Err(ApiError::classify(format!(
                "stale conversation_id {expected}; agent {agent} is bound to {canonical_agent}"
            )));
        }
    }
    let thread = if canonical_agent.is_empty() {
        empty_thread(params)
    } else {
        history_detail(view, params, method, canonical_agent)?
    };
    if method == "workspace.get" {
        answer["run"]["thread"] = thread.clone();
    }
    answer["thread"] = thread;
    Ok(answer)
}

fn history_detail(
    view: &ReadView,
    params: &Value,
    method: &str,
    agent: &str,
) -> Result<Value, ApiError> {
    let history = view
        .history(agent)
        .ok_or_else(|| ApiError::internal(format!("conversation snapshot missing: {agent}")))?;
    Ok(if method == "task.get" {
        history.render_task(params)
    } else {
        history.render(params)
    })
}

#[cfg(test)]
fn workspace_detail(
    view: &ReadView,
    params: &Value,
    owner: &str,
    answer: Value,
) -> Result<Value, ApiError> {
    owner_detail(view, params, "workspace.get", owner, answer)
}

fn empty_thread(params: &Value) -> Value {
    let thread = crate::thread::Thread::default();
    if let Some(after) = crate::app::thread_cursor(params) {
        return thread.wire_value_after(after);
    }
    match crate::app::thread_detail(params) {
        crate::thread::ThreadDetail::Page(limit) => thread.wire_value_page(None, limit),
        _ => thread.wire_value(),
    }
}

fn optional_identity<'a>(params: &'a Value, name: &str) -> Result<Option<&'a str>, ApiError> {
    let value = params.get(name).and_then(Value::as_str);
    if value == Some("") {
        return Err(ApiError::invalid_params(format!("{name} cannot be empty")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::super::SnapshotDelta;
    use super::*;

    fn service() -> (tempfile::TempDir, SnapshotService) {
        let root = tempfile::tempdir().unwrap();
        let app = crate::app::AppState::new_unrooted(
            root.path().join("worktrees"),
            "main",
            true,
            "/tmp/snapshot-service.sock",
        );
        (root, app.snapshot_service())
    }

    #[test]
    fn workspace_list_distinguishes_an_empty_project_id_from_an_absent_filter() {
        let (_root, service) = service();
        service
            .view
            .publish_delta(SnapshotDelta::new(service.view.next_version()).put(
                "workspace.list",
                "",
                json!({"workspaces":[{"id":"one"}]}),
            ));
        assert_eq!(
            service
                .read("workspace.list", &json!({"project_id":""}))
                .unwrap(),
            json!({"workspaces":[]})
        );
    }

    #[test]
    fn snapshot_reads_keep_typed_parameter_and_result_checks() {
        let (_root, service) = service();
        let error = service
            .read("board.list", &json!({"extra":true}))
            .unwrap_err();
        assert_eq!(error.code(), "invalid_params");
        service
            .view
            .publish_delta(SnapshotDelta::new(service.view.next_version()).put(
                "board.list",
                "",
                json!({"invalid":true}),
            ));
        let error = service.read("board.list", &json!({})).unwrap_err();
        assert_eq!(error.code(), "internal");
        assert!(error.message().contains("does not match"));
    }

    #[test]
    fn missing_snapshot_scope_uses_the_existing_refusal_code() {
        let (_root, service) = service();
        for (method, params) in [
            ("workspace.get", json!({"workspace_id":"missing"})),
            ("agent.list", json!({"entity_id":"missing"})),
        ] {
            assert_eq!(
                service.read(method, &params).unwrap_err().code(),
                "not_found"
            );
        }
    }

    #[test]
    fn an_agentless_workspace_cursor_keeps_the_existing_refusal() {
        let view = ReadView::default();
        let row = json!({"thread":{"agent":{"id":""}}, "run":{}, "agents":[]});
        let result = workspace_detail(&view, &json!({"thread_after_sequence":0}), "workspace", row);
        assert_eq!(
            result.unwrap_err().message(),
            "no agent here yet — send a message to create one"
        );
    }

    #[test]
    fn a_workspace_without_an_owner_preserves_optional_identity_behavior() {
        let view = ReadView::default();
        let row = json!({"thread":null,"run":null,"agents":[]});
        let result = workspace_detail(
            &view,
            &json!({"conversation_id":""}),
            "workspace",
            row.clone(),
        );
        assert_eq!(result.unwrap(), row);
        let result = workspace_detail(&view, &json!({"agent_id":""}), "workspace", row);
        assert_eq!(result.unwrap_err().code(), "not_found");
    }

    #[test]
    fn initialized_v1_dispatch_uses_the_published_read_view() {
        let root = tempfile::tempdir().unwrap();
        let mut app = crate::app::AppState::new_unrooted(
            root.path().join("worktrees"),
            "main",
            true,
            "/tmp/v1-snapshot.sock",
        );
        app.initialize_snapshot_projection();
        let mut board = app.read_view.load().get_owned("board.list", "").unwrap();
        board["snapshot_marker"] = json!(true);
        app.read_view
            .publish_delta(SnapshotDelta::new(app.read_view.next_version()).put(
                "board.list",
                "",
                board,
            ));
        let actual = crate::api::v1::dispatch(&mut app, "board.list", &json!({}))
            .unwrap()
            .unwrap();
        assert_eq!(actual["snapshot_marker"], true);
    }

    struct ChangingCatalog(std::sync::atomic::AtomicBool);

    impl crate::harness::installed::CliProbe for ChangingCatalog {
        fn read(&self, binary: &str) -> crate::harness::installed::CliReading {
            if binary != "codex" {
                return Default::default();
            }
            let id = if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                "fresh-model"
            } else {
                "old-model"
            };
            crate::harness::installed::CliReading {
                version: Some(semver::Version::parse("0.155.1").unwrap()),
                listed: Some(vec![crate::harness::installed::ListedModel {
                    id: id.into(),
                    label: id.into(),
                    hidden: false,
                    efforts: vec![],
                }]),
            }
        }
    }

    #[test]
    fn models_snapshot_observes_new_offers_without_a_greeting_or_app_lock() {
        use crate::harness::installed::{Readings, CODEX_MODEL_LIST};
        use std::sync::{atomic::Ordering, Arc, Mutex};
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let fake = Box::leak(Box::new(ChangingCatalog(
            std::sync::atomic::AtomicBool::new(false),
        )));
        let readings = Readings::answering_inline(fake);
        let mut app = crate::app::AppState::new_unrooted(
            root.path().join("worktrees"),
            "main",
            true,
            "/tmp/catalog-snapshot.sock",
        )
        .with_cli_readings(Arc::clone(&readings));
        app.default_harness = crate::models::AgentProvider::Codex;
        app.initialize_snapshot_projection();
        let service = app.snapshot_service();
        assert_eq!(
            service.read("models.list", &json!({})).unwrap()["models"][0]["id"],
            "old-model"
        );
        let state = Arc::new(Mutex::new(app));
        let guard = state.lock().unwrap();
        fake.0.store(true, Ordering::SeqCst);
        readings.fresh_reading("codex", &CODEX_MODEL_LIST, Duration::ZERO);
        let (sent, received) = std::sync::mpsc::channel();
        let reader =
            std::thread::spawn(move || sent.send(service.read("models.list", &json!({}))).unwrap());
        let observed = received.recv_timeout(Duration::from_secs(2));
        drop(guard);
        reader.join().unwrap();
        let observed = observed
            .expect("catalog snapshot does not wait for app lock")
            .unwrap();
        assert_eq!(observed["models"][0]["id"], "fresh-model");
        let provider = observed["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["id"] == "codex")
            .unwrap();
        assert_eq!(provider["models"][0]["id"], "fresh-model");
    }
}
