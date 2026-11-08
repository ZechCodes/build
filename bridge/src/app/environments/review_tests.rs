use super::edit_tests::{fixture, ok};
use crate::app::{AppState, OffLockGate};
use crate::app::runtime::deferred::DeferredNext;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn environment_mutation_probes_leave_the_app_lock_free() {
    for (method, params) in [
        ("environment.rename", json!({"environment_id":"env-1","name":"New"})),
        ("environment.set_isolation", json!({"environment_id":"env-1","isolation":"worktree"})),
        ("project.rename", json!({"project_id":"proj-1","name":"New"})),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(root.path());
        let (gate, control) = OffLockGate::new();
        app.off_lock_environment_probe_gate = Some(gate);
        let (answer, job) = app.dispatch_deferring(method, &params);
        assert!(answer.is_ok(), "{answer:?}");
        let job = job.expect("a mutation must defer its repository and availability probes");
        let state = Arc::new(Mutex::new(app));
        let worker = std::thread::spawn(move || job.run());
        control.wait_for_arrival();
        {
            let app = state.try_lock().expect("a blocked probe must leave the app lock available");
            assert_eq!(app.projects.environment("env-1").unwrap().name, "Original");
            assert_eq!(app.projects.get("proj-1").unwrap().name, "First");
        }
        control.release();
        let done = worker.join().unwrap();
        let mut app = state.lock().unwrap();
        match app.apply_deferred_stage(method, &params, done) {
            DeferredNext::Answered(answer) => assert!(answer.is_ok(), "{answer:?}"),
            DeferredNext::Again(_) => panic!("configuration observation must settle after one probe"),
        }
    }
}

fn published_source(app: &AppState, project: &str, source: &Value) -> Value {
    let projects = app.read_view.load().get_owned("project.list", "").unwrap();
    let row = projects["projects"].as_array().unwrap().iter().find(|p|p["project_id"]==project).unwrap();
    row["sources"].as_array().unwrap().iter().find(|s|s["id"]==*source).unwrap().clone()
}

#[test]
fn environment_source_edits_publish_fresh_dependent_remotes_immediately() {
    let root = tempfile::tempdir().unwrap(); let mut app = fixture(root.path());
    let first = crate::git_fixture::init_repo_named(root.path(), "first");
    let second = crate::git_fixture::init_repo_named(root.path(), "second");
    git2::Repository::open(&first).unwrap().remote("origin", "https://example.invalid/first.git").unwrap();
    git2::Repository::open(&second).unwrap().remote("origin", "https://example.invalid/second.git").unwrap();
    app.initialize_snapshot_projection();
    let row = ok(&mut app, "environment.add_source", json!({"environment_id":"env-1","path":first}));
    let source = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    for project in ["proj-1", "proj-2"] {
        assert_eq!(published_source(&app, project, &source)["remote"], "https://example.invalid/first.git");
    }
    ok(&mut app, "environment.update_source", json!({"environment_id":"env-1","source_id":source,"path":second}));
    for project in ["proj-1", "proj-2"] {
        let row = published_source(&app, project, &source);
        assert_eq!(row["path"], json!(second));
        assert_eq!(row["remote"], "https://example.invalid/second.git");
    }
}

#[test]
fn orphan_environment_sources_sync_on_a_later_due_pass() {
    use crate::app::projects::base_sync::SyncPass;
    let root = tempfile::tempdir().unwrap(); let mut app = fixture(root.path());
    let git = crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(&mut app,"environment.add_source",json!({"environment_id":"env-1","path":git}));
    let id = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    app.projects.clear();
    ok(&mut app,"environment.sync_source",json!({"environment_id":"env-1","source_id":id}));
    let state = Arc::new(Mutex::new(app));
    AppState::sync_sources(&state, SyncPass::Requested, 1000);
    AppState::sync_sources(&state, SyncPass::Due, 400000);
    let app = state.lock().unwrap();
    let source = app.projects.environment("env-1").unwrap().sources.last().unwrap();
    assert_eq!(source.sync_status.as_ref().unwrap().last_attempt_ms, 400000);
}
