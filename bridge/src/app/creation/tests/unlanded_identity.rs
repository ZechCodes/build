use super::admission_recovery::{crash_in_project_step, register_legacy, SimulatedCrash};
use super::*;

fn assert_unrelated_is_not_fenced(f: &Fixture) {
    let guard = f.state.lock().unwrap().refuse_pending_creation("proj-2");
    let operation_id = operation(131);
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-2","name":"probe"}),
        WriteMetadata {
            operation_id: operation_id.clone(),
            after: None,
        },
    )
    .unwrap();
    let claimed = f
        .store
        .writer()
        .claim_mutation(PRINCIPAL, &operation_id)
        .unwrap();
    assert!(
        guard.is_ok() && claimed,
        "unrelated project: pending guard={guard:?}, claim={claimed}"
    );
    // The probe only claimed SQL ownership; no executor/filesystem work ran.
    f.store
        .writer()
        .finish_creation(
            PRINCIPAL,
            &operation_id,
            json!({
                "state":"failed","effect":"unapplied",
                "error":{"code":"probe_finished","message":"No effects ran","retryable":false}
            }),
        )
        .unwrap();
}

fn reopen_reuse_and_reconcile(f: Fixture, id: &str) {
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    assert_eq!(f.record(id).state, "unknown");
    assert_eq!(f.record(id).execution["project_id"], "proj-2");
    register_legacy(&f.state, f._root.path());
    let marker = f._root.path().join("legacy/owned");
    std::fs::write(&marker, b"legacy-owned").unwrap();
    let legacy = {
        let app = f.state.lock().unwrap();
        let project = app.projects.get("proj-2").unwrap();
        assert_ne!(project.project_key, f.record(id).execution["project_key"]);
        app.project_json(project)
    };
    assert_unrelated_is_not_fenced(&f);
    f.run(id);
    f.run(id);
    let record = f.record(id);
    assert_eq!(record.state, "succeeded", "{record:?}");
    assert_eq!(record.execution["project_id"], "proj-3");
    let app = f.state.lock().unwrap();
    assert_eq!(app.projects.len(), 3);
    assert_eq!(
        app.project_json(app.projects.get("proj-2").unwrap()),
        legacy
    );
    assert!(app.workspaces.list(Some("proj-2")).is_empty());
    assert_eq!(app.workspaces.list(Some("proj-3")).len(), 1);
    assert_eq!(std::fs::read(marker).unwrap(), b"legacy-owned");
}

#[test]
fn pre_config_crash_does_not_fence_reused_project_id() {
    let f = Fixture::new();
    let id = operation(130);
    f.admit(&id);
    let _observer = observe_boundary(|boundary| {
        if boundary == Boundary::ProjectIdentityCheckpointed {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    crash_in_project_step(&f, &id);
    assert_eq!(f.record(&id).execution["project_id"], "proj-2");
    reopen_reuse_and_reconcile(f, &id);
}

#[test]
fn config_and_terminal_write_failures_do_not_fence_reused_project_id() {
    let f = Fixture::new();
    let id = operation(132);
    f.admit(&id);
    let blocker = f._root.path().join("config-blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    f.state.lock().unwrap().config_path = Some(blocker.join("config.json"));
    let store = f.store.clone();
    let _observer = observe_boundary(move |boundary| {
        if boundary == Boundary::BeforeFailureTerminal {
            store.fail_next_write();
        }
    });
    let error = execute(&f.state, f.record(&id)).unwrap_err();
    assert!(error.contains("injected store failure"), "{error}");
    drop(_observer);
    let pending = f.record(&id);
    assert_eq!(pending.state, "executing");
    assert!(pending.outcome.is_none());
    assert_eq!(
        f.state.lock().unwrap().projects.len(),
        1,
        "config save did not land"
    );
    reopen_reuse_and_reconcile(f, &id);
}

#[test]
fn failed_workspace_attempt_does_not_report_preexisting_project_as_landed() {
    let f = Fixture::new();
    let id = operation(133);
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":"proj-1","name":"failed"}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    let home = f
        .state
        .lock()
        .unwrap()
        .projects
        .get("proj-1")
        .unwrap()
        .home_path
        .clone();
    std::fs::remove_dir_all(home).unwrap();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed");
    assert!(record.execution["steps"]
        .as_array()
        .unwrap()
        .iter()
        .all(|step| step["step"] != "project"));
    assert_eq!(record.outcome.as_ref().unwrap()["effect"], "unapplied");
    assert!(record.outcome.unwrap()["error"]["details"]
        .get("project")
        .is_none());
}
