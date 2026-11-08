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
    fn reopen(self) -> Self {
        let Self {
            _root,
            state,
            store,
        } = self;
        drop(state);
        drop(store);
        let context =
            HarnessContext::resolved(_root.path().join("mcp.sock"), _root.path().to_path_buf())
                .unwrap();
        let app = AppState::new_unrooted_configured(
            _root.path().join("workspaces"),
            "main",
            true,
            context,
        )
        .with_config(_root.path().join("config.json"))
        .unwrap()
        .with_task_store(_root.path().join("tasks"))
        .unwrap();
        app.initialize_read_context();
        let store = app.store.clone().unwrap();
        Self {
            _root,
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
    let f = f.reopen();
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

#[test]
fn restart_after_workspace_publication_keeps_plain_copy_and_edits() {
    let f = Fixture::new();
    let id = operation(80);
    f.admit(&id);
    f.run(&id);
    let mut record = f.record(&id);
    let workspace: crate::workspace::Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    std::fs::write(workspace.directories[0].path.join("source"), b"edited").unwrap();
    // Recreate the precise interruption window: Ready manifest and committed
    // checkpoints, no terminal journal outcome or outbox entry yet.
    record.state = "executing".into();
    record.outcome = None;
    {
        let mut connection = f.store.writer_for_test();
        let tx = connection.transaction().unwrap();
        tx.execute(
            "DELETE FROM mutation_outbox WHERE command_id=?1",
            [record.command_id],
        )
        .unwrap();
        tx.execute(
            "UPDATE mutation_commands SET state='executing',record=?1 WHERE command_id=?2",
            rusqlite::params![json!(record).to_string(), record.command_id],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let f = f.reopen();
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(
        std::fs::read(workspace.directories[0].path.join("source")).unwrap(),
        b"edited"
    );
    assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
}

#[cfg(unix)]
#[test]
fn creation_residue_is_nonterminal_until_owned_cleanup_succeeds() {
    let f = Fixture::new();
    let id = operation(81);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    let mut evidence = project_step(&f.state, &record).unwrap();
    let workspace = f
        .state
        .lock()
        .unwrap()
        .reserve_creation_workspace("proj-2", &json!({"name":"residue"}))
        .unwrap();
    f.state
        .lock()
        .unwrap()
        .workspaces
        .claim_creation_workspace(&workspace)
        .unwrap();
    evidence["workspace"] = json!(workspace);
    evidence["residue"] = json!({"paths":[workspace.root],"error":"injected cleanup failure"});
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "reconciling", evidence)
        .unwrap();
    assert!(f.store.writer().finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"creation_failed","message":"failed","retryable":false}})).is_err());
    let foreign = f._root.path().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("keep"), b"kept").unwrap();
    std::os::unix::fs::symlink(&foreign, &workspace.directories[0].path).unwrap();
    f.run(&id);
    assert_eq!(f.record(&id).state, "reconciling");
    assert!(f.record(&id).outcome.is_none());
    assert_eq!(std::fs::read(foreign.join("keep")).unwrap(), b"kept");
    std::fs::remove_file(&workspace.directories[0].path).unwrap();
    f.run(&id);
    assert_eq!(f.record(&id).state, "failed");
    assert!(!workspace.root.exists());
}

#[tokio::test]
async fn creation_frame_acknowledges_then_runs_without_request_lifetime() {
    let f = Fixture::new();
    let id = operation(82);
    let handler = AppState::handler(f.state.clone());
    let (sender, _output, _key) = crate::carrier::SessionSender::observable("creation");
    let frame = crate::transport::Frame {
        session_id: "creation".into(),
        message_id: "create".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "test".into(),
        payload: json!({"id":"create","method":"environment.create_project","params":f.params(),"write":{"operation_id":id}}),
    };
    let reply = handler.call(sender, frame);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["write"]["state"], "accepted");
    drop(handler);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while f.record(&id).state != "succeeded" {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn discard_unused_keeps_environment_sources_and_refuses_tasks() {
    let f = Fixture::new();
    let id = operation(83);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    project_step(&f.state, &record).unwrap();
    f.store.writer().finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace refused","retryable":false}})).unwrap();
    let params = json!({"project_id":"proj-2","creation_operation_id":id});
    let task = f
        .state
        .lock()
        .unwrap()
        .dispatch(
            "tasks.create",
            &json!({"project_id":"proj-2","title":"retain me"}),
        )
        .unwrap();
    assert!(task["task"].is_object(), "{task}");
    let error = f
        .state
        .lock()
        .unwrap()
        .project_discard_unused(&params)
        .unwrap_err();
    assert!(error.contains("task"), "{error}");
    let f = Fixture::new();
    let id = operation(84);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    project_step(&f.state, &record).unwrap();
    f.store.writer().finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace refused","retryable":false}})).unwrap();
    f.state
        .lock()
        .unwrap()
        .project_discard_unused(&json!({"project_id":"proj-2","creation_operation_id":id}))
        .unwrap();
    let app = f.state.lock().unwrap();
    assert_eq!(app.projects.len(), 1);
    assert!(app.projects.environment("env-1").is_some());
    assert!(app
        .projects
        .get("proj-1")
        .unwrap()
        .home_path
        .join("source")
        .exists());
}

#[test]
fn workspace_attempt_has_only_workspace_steps_and_links_landed_project() {
    let f = Fixture::new();
    let prior = operation(85);
    let record = f.admit(&prior);
    f.store.writer().claim_mutation(PRINCIPAL, &prior).unwrap();
    project_step(&f.state, &record).unwrap();
    f.store.writer().finish_creation(PRINCIPAL,&prior,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace refused","retryable":false}})).unwrap();
    let id = operation(86);
    let admitted = admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-2","name":"retry","continues":prior}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    assert!(admitted.execution["workspace"]["id"].is_string());
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "succeeded", "{record:?}");
    assert!(record.execution["steps"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["step"] != "project"));
    assert_eq!(record.outcome.unwrap()["result"]["project_id"], "proj-2");
}

#[cfg(unix)]
#[test]
fn creation_cleanup_reservation_blocks_legacy_workspace_retry() {
    let f = Fixture::new();
    let id = operation(90);
    let record = f.admit(&id);
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    let mut evidence = project_step(&f.state, &record).unwrap();
    let mut workspace = f
        .state
        .lock()
        .unwrap()
        .reserve_creation_workspace("proj-2", &json!({"name":"residue"}))
        .unwrap();
    workspace.status = crate::workspace::WorkspaceStatus::Failed;
    {
        let mut app = f.state.lock().unwrap();
        app.workspaces.claim_creation_workspace(&workspace).unwrap();
        app.workspaces
            .retain_creation_residue(workspace.clone())
            .unwrap();
    }
    evidence["workspace"] = json!(workspace);
    evidence["residue"] = json!({"paths":[workspace.root],"error":"cleanup needs retry"});
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "reconciling", evidence)
        .unwrap();
    let foreign = f._root.path().join("foreign-retry");
    std::fs::create_dir(&foreign).unwrap();
    std::os::unix::fs::symlink(&foreign, &workspace.directories[0].path).unwrap();
    // Startup restores the creation reservation before ordinary writes enter.
    let _handler = AppState::handler(f.state.clone());
    let mut app = f.state.lock().unwrap();
    assert!(app
        .workspace_retry(&json!({"workspace_id":workspace.id}))
        .is_err());
    assert!(app
        .workspace_set_locked(&json!({"workspace_id":workspace.id,"locked":true}))
        .is_err());
}
