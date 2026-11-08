use super::edit_tests::ok;
use crate::app::runtime::deferred::DeferredNext;
use crate::app::{AppState, ConfigPersistStep, OffLockGate};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn fixture(root: &Path) -> (AppState, PathBuf) {
    let upstream = crate::git_fixture::init_repo_named(root, "upstream");
    let checkout = root.join("checkout");
    crate::git_fixture::git_in(
        root,
        &["clone", "-q", upstream.to_str().unwrap(), "checkout"],
    );
    for name in ["home", "orphan-home"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let environments = ["env-1", "orphan"].map(|id| json!({
        "id":id,"name":id,"sources":[
            {"id":"source-1","name":"home","mount":"home","path":root.join(if id=="orphan" {"orphan-home"} else {"home"})},
            {"id":"source-2","name":"shared","mount":"shared","path":checkout}
        ]
    }));
    let config = root.join("config.json");
    std::fs::write(&config, json!({"environments":environments,"projects":[
        {"id":"proj-1","environment_id":"env-1","project_key":"opaque","path":root.join("home"),"added_sources":[]}
    ]}).to_string()).unwrap();
    let mut app = super::runtime_tests::state(root)
        .with_config(config)
        .unwrap();
    app.initialize_snapshot_projection();
    (app, checkout)
}

fn assert_pushed_remotes(app: &AppState, expected: &Path) {
    let view = app.read_view.load();
    let environments = view.get_owned("environment.list", "").unwrap();
    for row in environments["environments"].as_array().unwrap() {
        assert_eq!(
            row["sources"][1]["remote"],
            json!(expected),
            "environment {}",
            row["environment_id"]
        );
    }
    let projects = view.get_owned("project.list", "").unwrap();
    assert_eq!(
        projects["projects"][0]["sources"][1]["remote"],
        json!(expected)
    );
}

#[test]
fn delayed_environment_rename_preserves_newer_shared_remote_evidence() {
    for environment in ["orphan", "env-1"] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = fixture(root.path());
        let params = json!({"environment_id":environment,"name":"Renamed"});
        let (answer, job) = app.dispatch_deferring("environment.rename", &params);
        assert!(answer.is_ok());
        let delayed = job.unwrap().run();
        let next = crate::git_fixture::init_repo_named(root.path(), "next-origin");
        ok(
            &mut app,
            "project.update_source",
            json!({"project_id":"proj-1","source_id":"source-2","remote":next}),
        );
        assert_pushed_remotes(&app, &next);
        app.apply_deferred("environment.rename", &params, delayed)
            .unwrap();
        assert_pushed_remotes(&app, &next);
    }
}

#[test]
fn refused_environment_source_commit_preserves_collected_and_newer_git_evidence() {
    for (stale, failure) in [
        (true, None),
        (false, Some(ConfigPersistStep::Write)),
        (false, Some(ConfigPersistStep::Rename)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, checkout) = fixture(root.path());
        let next = crate::git_fixture::init_repo_named(root.path(), "edited-origin");
        let params = json!({"environment_id":"orphan","source_id":"source-2","remote":next,"name":"Unsaved"});
        let (answer, job) = app.dispatch_deferring("environment.update_source", &params);
        assert!(answer.is_ok());
        let probe = match app.apply_deferred_stage(
            "environment.update_source",
            &params,
            job.unwrap().run(),
        ) {
            DeferredNext::Again(probe) => probe,
            _ => panic!("source edit must collect post-Git evidence off-lock"),
        };
        let observed = probe.run();
        let expected = if stale {
            let newer = crate::git_fixture::init_repo_named(root.path(), "external-origin");
            git2::Repository::open(&checkout)
                .unwrap()
                .remote_set_url("origin", newer.to_str().unwrap())
                .unwrap();
            // An external Git writer plus maintenance can observe new facts even
            // while the source edit retains its own repository lock.
            let refresh = app.capture_project_snapshot_refresh().render();
            app.install_project_snapshot_refresh(refresh);
            app.projects.environment_mut("orphan").unwrap().name = "Concurrent".into();
            newer
        } else {
            next
        };
        app.config_persist_failure = failure;
        assert!(app
            .apply_deferred("environment.update_source", &params, observed)
            .is_err());
        assert_eq!(
            git2::Repository::open(&checkout)
                .unwrap()
                .find_remote("origin")
                .unwrap()
                .url(),
            expected.to_str()
        );
        assert_pushed_remotes(&app, &expected);
    }
}

#[test]
fn complete_repository_publication_leaves_the_app_lock_free() {
    for method in ["git.fetch", "project.update_source"] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, checkout) = fixture(root.path());
        let (gate, control) = OffLockGate::new();
        app.off_lock_repository_publication_gate = Some(gate);
        let state = app.shared();
        let params = if method == "git.fetch" {
            json!({"project_id":"proj-1","source_id":"source-2","source_path":checkout})
        } else {
            json!({"project_id":"proj-1","source_id":"source-2","remote":root.path().join("upstream")})
        };
        let (answer, job) = state.lock().unwrap().dispatch_deferring(method, &params);
        assert!(answer.is_ok());
        let worker_state = state.clone();
        let worker = std::thread::spawn(move || {
            let done = job.unwrap().run();
            worker_state
                .lock()
                .unwrap()
                .apply_deferred(method, &params, done)
        });
        control.wait_for_arrival();
        let progressed = state.try_lock().is_ok();
        control.release();
        worker.join().unwrap().unwrap();
        assert!(progressed,"repository fan-out discovery and project observations must also run off-lock: {method}");
    }
}

#[test]
fn delayed_source_add_preserves_newer_compatible_remote_evidence() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = fixture(root.path());
    let added = crate::git_fixture::init_repo_named(root.path(), "added");
    let added_origin = root.path().join("upstream");
    git2::Repository::open(&added)
        .unwrap()
        .remote("origin", added_origin.to_str().unwrap())
        .unwrap();
    let params = json!({"environment_id":"env-1","path":added});
    let (answer, job) = app.dispatch_deferring("environment.add_source", &params);
    assert!(answer.is_ok());
    let probe =
        match app.apply_deferred_stage("environment.add_source", &params, job.unwrap().run()) {
            DeferredNext::Again(probe) => probe,
            _ => panic!("source addition must render prospective references off-lock"),
        };
    let delayed = probe.run();
    let next = crate::git_fixture::init_repo_named(root.path(), "next-origin");
    ok(
        &mut app,
        "project.update_source",
        json!({"project_id":"proj-1","source_id":"source-2","remote":next}),
    );
    app.apply_deferred("environment.add_source", &params, delayed)
        .unwrap();
    assert_pushed_remotes(&app, &next);
    let projects = app.read_view.load().get_owned("project.list", "").unwrap();
    assert_eq!(
        projects["projects"][0]["sources"][2]["remote"],
        json!(added_origin)
    );
}
