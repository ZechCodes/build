use super::*;
use std::sync::Barrier;

fn concurrent_admissions(method: &'static str, params: Value) {
    let f = Fixture::new();
    let id = operation(110);
    let barrier = Arc::new(Barrier::new(2));
    let records = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let state = f.state.clone();
                let barrier = barrier.clone();
                let params = params.clone();
                let id = id.clone();
                scope.spawn(move || {
                    let _observer = observe_boundary(move |boundary| {
                        if boundary == Boundary::AdmissionPrepared {
                            barrier.wait();
                        }
                    });
                    admit(
                        &state,
                        method,
                        params,
                        WriteMetadata {
                            operation_id: id,
                            after: None,
                        },
                    )
                })
            })
            .collect();
        threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(records.iter().all(Result::is_ok), "{records:?}");
    let first = records[0].as_ref().unwrap();
    let second = records[1].as_ref().unwrap();
    assert_eq!(
        first, second,
        "both calls receive the winning acknowledgement"
    );
    assert_eq!(f.store.writer().pending_creations().unwrap().len(), 1);
    f.run(&id);
    f.run(&id);
    assert_eq!(f.record(&id).state, "succeeded");
    assert_eq!(f.state.lock().unwrap().workspaces.list(None).len(), 1);
    let mut changed = first.params.clone();
    changed["name"] = json!("a different intent");
    assert!(admit(
        &f.state,
        method,
        changed,
        WriteMetadata {
            operation_id: id,
            after: None,
        },
    )
    .is_err());
}

#[test]
fn concurrent_create_project_same_operation_reuses_winning_ack() {
    concurrent_admissions(
        "environment.create_project",
        json!({"environment_id":"env-1","name":"new project","workspace":{"name":"first"}}),
    );
}

#[test]
fn concurrent_workspace_same_operation_reuses_winning_ack() {
    concurrent_admissions(
        "workspace.create",
        json!({"project_id":"proj-1","name":"first"}),
    );
}

fn register_legacy(state: &Arc<Mutex<AppState>>, home: &std::path::Path) {
    let path = home.join("legacy");
    std::fs::create_dir(&path).unwrap();
    let mut app = state.lock().unwrap();
    let id = app.add_project(path, "main".into());
    assert_eq!(id, "proj-2");
    app.persist();
}

fn assert_reallocated_creation(f: &Fixture, id: &str) {
    let record = f.record(id);
    assert_eq!(record.state, "succeeded", "{record:?}");
    assert_eq!(record.execution["project_id"], "proj-3");
    let app = f.state.lock().unwrap();
    assert_eq!(app.projects.len(), 3);
    assert_eq!(
        app.projects.get("proj-3").unwrap().project_key,
        record.execution["project_key"]
    );
    assert_ne!(
        app.projects.get("proj-2").unwrap().project_key,
        record.execution["project_key"]
    );
    assert_eq!(app.workspaces.list(Some("proj-3")).len(), 1);
    assert!(app.workspaces.list(Some("proj-2")).is_empty());
}

#[test]
fn legacy_registration_after_project_checkpoint_reallocates() {
    let f = Fixture::new();
    let id = operation(111);
    f.admit(&id);
    let state = f.state.clone();
    let root = f._root.path().to_path_buf();
    let _observer = observe_boundary(move |boundary| {
        if boundary == Boundary::ProjectCheckpointed {
            register_legacy(&state, &root);
        }
    });
    f.run(&id);
    assert_reallocated_creation(&f, &id);
}

#[derive(Debug)]
struct SimulatedCrash;

fn crash_in_project_step(f: &Fixture, id: &str) {
    let record = f.record(id);
    assert!(f.store.writer().claim_mutation(PRINCIPAL, id).unwrap());
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        project_step(&f.state, &record).unwrap();
    }));
    assert!(crash.unwrap_err().is::<SimulatedCrash>());
}

#[test]
fn restart_after_project_id_checkpoint_reallocates() {
    let f = Fixture::new();
    let id = operation(112);
    f.admit(&id);
    let _observer = observe_boundary(|boundary| {
        if boundary == Boundary::ProjectCheckpointed {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    crash_in_project_step(&f, &id);
    register_legacy(&f.state, f._root.path());
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    f.run(&id);
    assert_reallocated_creation(&f, &id);
}

pub(super) fn crash_before_post_project_checkpoint() {
    let f = Fixture::new();
    let id = operation(3);
    let accepted = f.admit(&id);
    let state = f.state.clone();
    let root = f._root.path().to_path_buf();
    let _observer = observe_boundary(move |boundary| match boundary {
        Boundary::ProjectCheckpointed => register_legacy(&state, &root),
        Boundary::ProjectPersisted => std::panic::panic_any(SimulatedCrash),
        _ => {}
    });
    crash_in_project_step(&f, &id);
    let saved = f.record(&id);
    assert_eq!(saved.execution["project_id"], "proj-2");
    assert_eq!(
        saved.execution["steps"],
        json!([]),
        "post-project checkpoint was never written"
    );
    assert_eq!(
        saved.execution["project_key"],
        accepted.execution["project_key"]
    );
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    assert_eq!(f.record(&id).state, "unknown");
    f.run(&id);
    assert_reallocated_creation(&f, &id);
    f.run(&id);
    assert_reallocated_creation(&f, &id);
}
