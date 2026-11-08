use super::edit_tests::{fixture, ok};
use crate::app::runtime::deferred::DeferredNext;
use crate::app::OffLockGate;
use serde_json::json;
use std::sync::mpsc;
use std::time::Duration;

fn source_add_progress(clone_settlement: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let existing = crate::git_fixture::init_repo_named(root.path(), "existing");
    git2::Repository::open(&existing)
        .unwrap()
        .remote("origin", "https://example.invalid/existing.git")
        .unwrap();
    ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":existing}),
    );
    let origin = crate::git_fixture::init_repo_named(root.path(), "new-origin");
    app.initialize_snapshot_projection();
    let (gate, control) = OffLockGate::new();
    if clone_settlement {
        app.off_lock_environment_source_clone_gate = Some(gate);
    } else {
        app.off_lock_environment_source_admission_gate = Some(gate);
    }
    let state = app.shared();
    let worker_state = state.clone();
    let worker = std::thread::spawn(move || {
        let params = json!({"environment_id":"env-1","remote":origin});
        let (answer, job) = worker_state
            .lock()
            .unwrap()
            .dispatch_deferring("environment.add_source", &params);
        assert!(answer.is_ok(), "{answer:?}");
        let mut job = job.unwrap();
        loop {
            let done = job.run();
            match worker_state.lock().unwrap().apply_deferred_stage(
                "environment.add_source",
                &params,
                done,
            ) {
                DeferredNext::Again(next) => job = next,
                DeferredNext::Answered(answer) => return answer,
            }
        }
    });
    control.wait_for_arrival();
    let reader_state = state.clone();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let answer = reader_state
            .lock()
            .unwrap()
            .dispatch("environment.list", &json!({}));
        send.send(answer.is_ok()).unwrap();
    });
    let progressed = receive.recv_timeout(Duration::from_secs(3)).ok();
    control.release();
    worker.join().unwrap().unwrap();
    reader.join().unwrap();
    assert_eq!(progressed,Some(true),"a second app-lock RPC must complete while repository discovery is blocked; clone settlement={clone_settlement}");
}

#[test]
fn environment_source_admission_repository_probe_leaves_app_lock_free() {
    source_add_progress(false);
}

#[test]
fn environment_source_clone_settlement_repository_probe_leaves_app_lock_free() {
    source_add_progress(true);
}

#[test]
fn environment_source_add_revalidates_mount_claims_after_discovery() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let params =
        json!({"environment_id":"env-1","path":root.path().join("target"),"name":"collision"});
    let (answer, job) = app.dispatch_deferring("environment.add_source", &params);
    assert!(answer.is_ok());
    let mut done = job.unwrap().run();
    app.projects.environment_mut("env-1").unwrap().sources[0].mount = "collision".into();
    app.projects.refresh_environment_projects("env-1");
    loop {
        match app.apply_deferred_stage("environment.add_source", &params, done) {
            DeferredNext::Again(job) => done = job.run(),
            DeferredNext::Answered(result) => {
                if result.is_ok() {
                    let sources = &app.projects.environment("env-1").unwrap().sources;
                    let mounts = sources
                        .iter()
                        .map(|s| &s.mount)
                        .collect::<std::collections::BTreeSet<_>>();
                    assert_eq!(
                        mounts.len(),
                        sources.len(),
                        "a concurrent membership edit must never create duplicate mounts"
                    );
                }
                break;
            }
        }
    }
}

#[test]
fn environment_clone_refuses_a_remote_claim_added_during_discovery() {
    for after_discovery in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut app = fixture(root.path());
        let existing = crate::git_fixture::init_repo_named(root.path(), "existing");
        git2::Repository::open(&existing)
            .unwrap()
            .remote("origin", "https://example.invalid/old.git")
            .unwrap();
        let row = ok(
            &mut app,
            "environment.add_source",
            json!({"environment_id":"env-1","path":existing}),
        );
        let source = row["sources"].as_array().unwrap().last().unwrap()["id"].clone();
        let count = row["sources"].as_array().unwrap().len();
        let origin = crate::git_fixture::init_repo_named(root.path(), "new-origin");
        app.initialize_snapshot_projection();
        let params = json!({"environment_id":"env-1","remote":origin});
        let (answer, admission) = app.dispatch_deferring("environment.add_source", &params);
        assert!(answer.is_ok());
        let opening = match app.apply_deferred_stage(
            "environment.add_source",
            &params,
            admission.unwrap().run(),
        ) {
            DeferredNext::Again(job) => job,
            _ => panic!("remote admission must hand opening to the off-lock drain"),
        };
        if !after_discovery {
            ok(
                &mut app,
                "project.update_source",
                json!({"project_id":"proj-1","source_id":source,"remote":origin}),
            );
        }
        let claims =
            match app.apply_deferred_stage("environment.add_source", &params, opening.run()) {
                DeferredNext::Again(job) => job,
                _ => panic!(
                    "clone settlement must hand origin claims to the off-lock publication probe"
                ),
            };
        let observed = claims.run();
        if after_discovery {
            // External Git plus maintenance can publish facts despite the
            // operation's repository locks. Commit must prefer those facts.
            git2::Repository::open(&existing)
                .unwrap()
                .remote_set_url("origin", origin.to_str().unwrap())
                .unwrap();
            let newer = app.capture_project_snapshot_refresh().render();
            app.install_project_snapshot_refresh(newer);
        }
        let error = app
            .apply_deferred("environment.add_source", &params, observed)
            .unwrap_err();
        assert!(
            error.contains("source remote is already registered"),
            "{error}"
        );
        assert_eq!(
            app.projects.environment("env-1").unwrap().sources.len(),
            count
        );
    }
}

#[test]
fn environment_clone_refuses_an_unreadable_registered_origin() {
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(root.path());
    let origin = crate::git_fixture::init_repo_named(root.path(), "origin");
    let existing = crate::git_fixture::init_repo_named(root.path(), "existing");
    git2::Repository::open(&existing)
        .unwrap()
        .remote("origin", origin.to_str().unwrap())
        .unwrap();
    let before = ok(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","path":existing}),
    );
    std::fs::rename(existing.join(".git"), existing.join(".git-away")).unwrap();
    let reply = super::runtime_tests::rpc(
        &mut app,
        "environment.add_source",
        json!({"environment_id":"env-1","remote":origin}),
    );
    std::fs::rename(existing.join(".git-away"), existing.join(".git")).unwrap();
    assert_eq!(
        reply["ok"], false,
        "unknown origin must not release the registered remote claim: {reply}"
    );
    assert_eq!(
        app.projects.environment("env-1").unwrap().sources.len(),
        before["sources"].as_array().unwrap().len()
    );
}
