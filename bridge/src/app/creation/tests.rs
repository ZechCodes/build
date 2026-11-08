use super::*;
use crate::harness::HarnessContext;
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    state: Arc<Mutex<AppState>>,
    store: crate::store::Store,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("source"), b"kept").unwrap();
        let context =
            HarnessContext::resolved(root.path().join("mcp.sock"), root.path().to_path_buf())
                .unwrap();
        let mut app = AppState::new_unrooted_configured(
            root.path().join("workspaces"),
            "main",
            true,
            context,
        )
        .with_task_store(root.path().join("tasks"))
        .unwrap()
        .with_config(root.path().join("config.json"))
        .unwrap();
        app.add_project(home, "main".into());
        app.persist();
        let store = app.store.clone().unwrap();
        app.initialize_read_context();
        Self {
            _root: root,
            state: Arc::new(Mutex::new(app)),
            store,
        }
    }
    fn params(&self) -> Value {
        json!({"environment_id":"env-1","name":"new project","workspace":{"name":"first"}})
    }
    fn admit(&self, id: &str) -> MutationRecord {
        admit(
            &self.state,
            "environment.create_project",
            self.params(),
            WriteMetadata {
                operation_id: id.into(),
                after: None,
            },
        )
        .unwrap()
    }
    fn record(&self, id: &str) -> MutationRecord {
        self.store
            .writer()
            .creation_record(PRINCIPAL, id)
            .unwrap()
            .unwrap()
    }
    fn run(&self, id: &str) {
        execute(&self.state, self.record(id)).unwrap();
    }
}
fn operation(n: u128) -> String {
    uuid::Uuid::from_u128(n).to_string()
}

#[test]
fn create_project_idempotent_by_operation_id() {
    let f = Fixture::new();
    let id = operation(1);
    f.admit(&id);
    f.run(&id);
    let first = f.record(&id);
    assert_eq!(first.state, "succeeded", "{first:?}");
    let again = f.admit(&id);
    assert_eq!(again.outcome, first.outcome);
    f.run(&id);
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
    assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
}
#[test]
fn create_project_mints_fresh_key_and_workspace() {
    let f = Fixture::new();
    let id = operation(2);
    f.admit(&id);
    f.run(&id);
    let app = f.state.lock().unwrap();
    let project = app.projects.get("proj-2").unwrap();
    assert!(project.project_key.starts_with("project:"));
    assert!(uuid::Uuid::parse_str(project.project_key.strip_prefix("project:").unwrap()).is_ok());
    assert_eq!(
        project.home_path,
        app.projects.get("proj-1").unwrap().home_path
    );
    assert_eq!(project.environment_id, "env-1");
    let workspace = app.workspaces.list(Some("proj-2"));
    assert_eq!(workspace.len(), 1);
    assert_eq!(
        std::fs::read(workspace[0].directories[0].path.join("source")).unwrap(),
        b"kept"
    );
    let result = f.record(&id).outcome.unwrap();
    assert_eq!(result["result"]["project"]["project_id"], "proj-2");
    assert_eq!(
        result["result"]["workspace"]["workspace_id"],
        workspace[0].id
    );
}
#[test]
fn crash_between_steps_reconciles_without_duplicate() {
    let f = Fixture::new();
    let id = operation(3);
    let record = f.admit(&id);
    assert!(f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap());
    let evidence = project_step(&f.state, &record).unwrap();
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
    // No post-project checkpoint: replay finds the config's reserved opaque key.
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "unknown", evidence)
        .unwrap();
    let record = f.record(&id);
    execute(&f.state, record).unwrap();
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
    assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
}
#[test]
fn unknown_outcome_blocks_conflicting_retry() {
    let f = Fixture::new();
    let id = operation(4);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "unknown", record.execution)
        .unwrap();
    let second = admit(
        &f.state,
        "environment.create_project",
        f.params(),
        WriteMetadata {
            operation_id: operation(5),
            after: None,
        },
    );
    assert!(second.is_err());
    assert_eq!(f.state.lock().unwrap().projects.len(), 1);
    assert!(f.record(&id).outcome.is_none());
}
#[test]
fn workspace_step_failure_reports_landed_project() {
    let f = Fixture::new();
    let id = operation(6);
    let mut params = f.params();
    params["workspace"]["isolation"] = json!("invalid");
    // Admission validates isolation before any effects.
    assert!(admit(
        &f.state,
        "environment.create_project",
        params,
        WriteMetadata {
            operation_id: id.clone(),
            after: None
        }
    )
    .is_err());
    f.admit(&id);
    let source = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-1")
        .unwrap()
        .home_path
        .clone();
    std::fs::remove_dir_all(source).unwrap();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.as_ref().unwrap()["effect"], "partial");
    assert_eq!(record.execution["project_id"], "proj-2");
    assert_eq!(record.execution["steps"][0]["state"], "done");
    assert_eq!(record.execution["steps"][1]["state"], "failed");
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
}
#[test]
fn discard_unused_refused_when_project_used() {
    let f = Fixture::new();
    let id = operation(7);
    f.admit(&id);
    f.run(&id);
    let error = f
        .state
        .lock()
        .unwrap()
        .project_discard_unused(&json!({"project_id":"proj-2","creation_operation_id":id}))
        .unwrap_err();
    assert!(error.contains("workspace"), "{error}");
    assert_eq!(f.state.lock().unwrap().projects.len(), 2);
}
