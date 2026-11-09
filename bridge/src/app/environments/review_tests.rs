use super::edit_tests::{fixture, ok};
use crate::app::runtime::deferred::DeferredNext;
use crate::app::{AppState, OffLockGate};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn environment_mutation_probes_leave_the_app_lock_free() {
    for (method, params) in [
        (
            "environment.rename",
            json!({"environment_id":"env-1","name":"New"}),
        ),
        (
            "environment.set_isolation",
            json!({"environment_id":"env-1","isolation":"worktree"}),
        ),
        (
            "project.rename",
            json!({"project_id":"proj-1","name":"New"}),
        ),
        (
            "environment.update_source",
            json!({"environment_id":"env-1","source_id":"source-2","name":"New"}),
        ),
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
            let app = state
                .try_lock()
                .expect("a blocked probe must leave the app lock available");
            assert_eq!(app.projects.environment("env-1").unwrap().name, "Original");
            assert_eq!(app.projects.get("proj-1").unwrap().name, "First");
        }
        control.release();
        let done = worker.join().unwrap();
        let mut app = state.lock().unwrap();
        match app.apply_deferred_stage(method, &params, done) {
            DeferredNext::Answered(answer) => assert!(answer.is_ok(), "{answer:?}"),
            DeferredNext::Again(_) => {
                panic!("configuration observation must settle after one probe")
            }
        }
    }
}

fn published_source(app: &AppState, project: &str, source: &Value) -> Value {
    let projects = app.read_view.load().get_owned("project.list", "").unwrap();
    let row = projects["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["project_id"] == project)
        .unwrap();
    row["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == *source)
        .unwrap()
        .clone()
}

#[test]
fn environment_source_edits_publish_fresh_dependent_remotes_immediately() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let first = crate::git_fixture::init_repo_named(root.path(), "first");
    let second = crate::git_fixture::init_repo_named(root.path(), "second");
    git2::Repository::open(&first)
        .unwrap()
        .remote("origin", "https://example.invalid/first.git")
        .unwrap();
    git2::Repository::open(&second)
        .unwrap()
        .remote("origin", "https://example.invalid/second.git")
        .unwrap();
    app.initialize_snapshot_projection();
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":first}),
    );
    let source = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    for project in ["proj-1", "proj-2"] {
        assert_eq!(
            published_source(&app, project, &source)["remote"],
            "https://example.invalid/first.git"
        );
    }
    ok(
        &mut app,
        "environment.update_source",
        json!({"environment_id":"env-1","source_id":source,"path":second}),
    );
    for project in ["proj-1", "proj-2"] {
        let row = published_source(&app, project, &source);
        assert_eq!(row["path"], json!(second));
        assert_eq!(row["remote"], "https://example.invalid/second.git");
    }
}

#[test]
fn orphan_environment_sources_sync_on_a_later_due_pass() {
    use crate::app::projects::base_sync::SyncPass;
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let git = crate::git_fixture::init_repo_named(root.path(), "git");
    let row = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":git}),
    );
    let id = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
    app.projects.clear();
    ok(
        &mut app,
        "environment.sync_source",
        json!({"environment_id":"env-1","source_id":id}),
    );
    let state = Arc::new(Mutex::new(app));
    AppState::sync_sources(&state, SyncPass::Requested, 1000);
    AppState::sync_sources(&state, SyncPass::Due, 400000);
    let app = state.lock().unwrap();
    let source = app
        .projects
        .environment("env-1")
        .unwrap()
        .sources
        .last()
        .unwrap();
    assert_eq!(source.sync_status.as_ref().unwrap().last_attempt_ms, 400000);
}

#[test]
fn environment_probe_revalidates_configuration_before_publication() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let params = json!({"environment_id":"env-1","name":"Proposed"});
    let (answer, job) = app.dispatch_deferring("environment.rename", &params);
    assert!(answer.is_ok());
    let done = job.expect("probe must defer").run();
    app.projects
        .rename_project("proj-1", "Concurrent".into())
        .unwrap();
    match app.apply_deferred_stage("environment.rename", &params, done) {
        DeferredNext::Answered(Err(error)) => {
            assert!(error.contains("configuration changed"), "{error}")
        }
        _ => panic!("stale configuration must not publish"),
    }
    assert_eq!(app.projects.environment("env-1").unwrap().name, "Original");
    assert_eq!(app.projects.get("proj-1").unwrap().name, "Concurrent");
}

#[test]
fn deferred_isolation_refusal_keeps_the_unavailable_code() {
    assert_eq!(
        crate::api::ApiError::classify(
            "Rift isolation is unavailable: probe refused; locked to worktrees".into()
        )
        .code(),
        "unavailable"
    );
}

#[test]
fn cached_project_remote_requires_both_source_id_and_path() {
    let root = tempfile::tempdir().unwrap();
    let app = fixture(root.path());
    let mut row = app
        .project_list_rows()
        .into_iter()
        .find(|p| p.project_id == "proj-1")
        .unwrap();
    let old_path = row.sources[1].path.clone();
    row.sources[1].path = root.path().join("target");
    let cached = json!({"sources":[{"id":"source-2","path":old_path,"remote":"https://example.invalid/old.git"}]});
    let rendered = row.render_observed(json!({"rift":false,"reason":null}), Some(&cached));
    assert_eq!(rendered["sources"][1]["remote"], Value::Null);
}

fn orphan_shared_checkout(
    root: &std::path::Path,
) -> (AppState, std::path::PathBuf, std::path::PathBuf) {
    use crate::git_fixture::{git_in, init_repo_named};
    let upstream = init_repo_named(root, "upstream");
    let checkout = root.join("checkout");
    git_in(
        root,
        &["clone", "-q", upstream.to_str().unwrap(), "checkout"],
    );
    let home = init_repo_named(root, "other-home");
    let config = root.join("config.json");
    std::fs::write(&config, json!({
        "environments":[
            {"id":"orphan","name":"Orphan","sources":[{"id":"source-1","path":checkout,"name":"shared","mount":"shared","sync_base":true}]},
            {"id":"live","name":"Live","sources":[{"id":"source-1","path":home,"name":"home","mount":"home"}]}
        ],
        "projects":[{"id":"proj-1","environment_id":"live","project_key":"other","path":home,
            "added_sources":[{"id":"source-2","path":checkout,"name":"shared","mount":"shared"}]}]
    }).to_string()).unwrap();
    let mut app = super::runtime_tests::state(root)
        .with_config(config)
        .unwrap();
    app.initialize_snapshot_projection();
    (app, checkout, upstream)
}

fn pushed_orphan_remote(app: &AppState) -> Value {
    let pushed = app
        .read_view
        .load()
        .get_owned("environment.list", "")
        .unwrap();
    let row = pushed["environments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["environment_id"] == "orphan")
        .unwrap();
    assert_eq!(row["project_ids"], json!([]));
    row["sources"][0]["remote"].clone()
}

#[test]
fn orphan_shared_remote_is_pushed_immediately_after_git_and_project_mutations() {
    for method in ["git.fetch", "project.update_source"] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, checkout, upstream) = orphan_shared_checkout(root.path());
        assert_eq!(pushed_orphan_remote(&app), json!(upstream));
        let mut params =
            json!({"project_id":"proj-1","source_id":"source-2","source_path":checkout});
        let expected = if method == "project.update_source" {
            params.as_object_mut().unwrap().remove("source_path");
            let next = crate::git_fixture::init_repo_named(root.path(), "next-origin");
            params["remote"] = json!(next);
            next
        } else {
            upstream
        };
        ok(&mut app, method, params);
        assert_eq!(pushed_orphan_remote(&app), json!(expected), "{method}");
    }
}

#[test]
fn orphan_shared_remote_is_pushed_immediately_after_timed_fetch() {
    use crate::app::projects::base_sync::SyncPass;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _, upstream) = orphan_shared_checkout(root.path());
    ok(
        &mut app,
        "environment.sync_source",
        json!({"environment_id":"orphan","source_id":"source-1"}),
    );
    let state = Arc::new(Mutex::new(app));
    AppState::sync_sources(&state, SyncPass::Requested, 1000);
    // The due pass must fetch after the immediate queue has drained.
    AppState::sync_sources(&state, SyncPass::Due, 400000);
    let app = state.lock().unwrap();
    let status = app.projects.environment("orphan").unwrap().sources[0]
        .sync_status
        .as_ref()
        .unwrap();
    assert_eq!(status.last_attempt_ms, 400000);
    assert_eq!(status.last_fetched_ms, Some(400000));
    assert_eq!(pushed_orphan_remote(&app), json!(upstream));
}

#[test]
fn older_git_observation_cannot_replace_a_newer_orphan_remote() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, checkout, _) = orphan_shared_checkout(root.path());
    let params = json!({"project_id":"proj-1","source_id":"source-2","source_path":checkout});
    let (answer, job) = app.dispatch_deferring("git.fetch", &params);
    assert!(answer.is_ok());
    let older = job.unwrap().run();
    let next = crate::git_fixture::init_repo_named(root.path(), "next-origin");
    git2::Repository::open(&checkout)
        .unwrap()
        .remote_set_url("origin", next.to_str().unwrap())
        .unwrap();
    ok(&mut app, "git.fetch", params.clone());
    assert_eq!(pushed_orphan_remote(&app), json!(next));
    app.apply_deferred("git.fetch", &params, older).unwrap();
    assert_eq!(pushed_orphan_remote(&app), json!(next));
}

#[test]
fn git_environment_observation_leaves_the_app_lock_free() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, checkout, upstream) = orphan_shared_checkout(root.path());
    let (gate, control) = OffLockGate::new();
    app.off_lock_environment_probe_gate = Some(gate);
    let params = json!({"project_id":"proj-1","source_id":"source-2","source_path":checkout});
    let (answer, job) = app.dispatch_deferring("git.fetch", &params);
    assert!(answer.is_ok());
    let state = Arc::new(Mutex::new(app));
    let worker = std::thread::spawn(move || job.unwrap().run());
    control.wait_for_arrival();
    assert!(
        state.try_lock().is_ok(),
        "the post-Git environment probe must run off-lock"
    );
    control.release();
    let done = worker.join().unwrap();
    let mut app = state.lock().unwrap();
    app.apply_deferred("git.fetch", &params, done).unwrap();
    assert_eq!(pushed_orphan_remote(&app), json!(upstream));
}
