//! Regressions for retirement, transactional pin safety and off-lock admission.
use super::*;
use super::environment_deletion::call;
use super::project_agent::workspace;
use super::tracker::{filed, tracked};

fn params(project: &str) -> Value {
    json!({"project_id":project,"confirm":true,"tasks":"delete"})
}
fn finished(app: &mut AppState, project: &str) -> crate::workspace::Workspace {
    let id = workspace(app, project, "Deletion phases");
    let mut w = app.workspaces.get(&id).unwrap().clone();
    w.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&w).unwrap();
    app.workspaces.reload().unwrap();
    w
}
fn live(app: &mut AppState, root: &Path, owner: &str, agent: &str) -> TabKey {
    let key = TabKey::agent(root, agent);
    app.session_registry.test_insert_tab(key.clone(), dictated_agent_tab(root, owner, agent,
        DictatedSession::reporting(crate::harness::AgentStatus::Waiting)));
    key
}
fn fail_checkpoint(app: &AppState, phase: &str) {
    app.store.as_ref().unwrap().writer_for_test().execute_batch(&format!(
        "CREATE TEMP TRIGGER fail_phase BEFORE UPDATE ON mutation_commands WHEN json_extract(NEW.record,'$.execution.phase')='{phase}' BEGIN SELECT RAISE(ABORT,'checkpoint failure'); END;"
    )).unwrap();
}

#[test]
fn project_deletion_retires_sessions_before_config_and_on_same_process_retry() {
    for phase in ["config", "config_removed"] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let sibling = app.add_environment_sibling_for_test(&project);
        let task = filed(&mut app, &project, "Retirement boundary");
        let w = finished(&mut app, &project);
        let source = app.projects.get(&project).unwrap().repo_path.clone();
        let own = live(&mut app, &source, &project, "delete-agent");
        let other = live(&mut app, &source, &sibling, "sibling-agent");
        app.config_path = Some(root.path().join("config.json"));
        app.persist();
        if phase == "config" { app.project_deletion_failure = Some("config"); }
        else { fail_checkpoint(&app, phase); }
        let failed = app.handle(req("project.delete", params(&project)));
        assert_eq!(failed["ok"], false, "{failed}");
        assert!(!app.session_registry.contains(&own), "retire before {phase}");
        assert!(app.session_registry.contains(&other), "shared source sibling must survive");
        assert!(w.root.exists());
        assert!(app.store.as_ref().unwrap().load_tracker_task(task["id"].as_str().unwrap()).unwrap().is_some());
        app.project_deletion_failure = None;
        if phase != "config" { app.store.as_ref().unwrap().writer_for_test().execute_batch("DROP TRIGGER fail_phase").unwrap(); }
        // Retry must reconstruct live sessions, even after registration removal.
        let resumed = live(&mut app, &w.root, &w.id, "retry-agent");
        call(&mut app, "project.delete", params(&project));
        assert!(!app.session_registry.contains(&resumed));
        assert!(app.session_registry.contains(&other));
        assert!(!w.root.exists());
    }
}

#[test]
fn project_deletion_history_checkpoint_rollback_preserves_real_review_pins() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let id = workspace(&mut app, &project, "Pinned review");
    let opened = call(&mut app, "tasks.review.open", json!({"workspace_id":id,
        "request_id":"pin-rollback","title":"Pinned","description":"Review"}));
    let task = opened["task"]["id"].as_str().unwrap();
    let store = app.store.as_ref().unwrap().clone();
    let saved = store.load_review(task).unwrap().unwrap();
    let pins: Vec<_> = saved.snapshots.iter().flat_map(|s| &s.directories)
        .filter_map(|d| d.common_git_dir.clone()).collect();
    let refs = |path: &Path| -> Vec<String> { git2::Repository::open_bare(path).unwrap().references_glob("refs/build/*").unwrap()
        .map(|r| { let r = r.unwrap(); format!("{}:{:?}",r.name().unwrap(),r.target()) }).collect() };
    let before: Vec<_> = pins.iter().map(|p| refs(p)).collect();
    assert!(before.iter().any(|p| !p.is_empty()));
    let mut w = app.workspaces.get(&id).unwrap().clone();
    w.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&w).unwrap();
    app.workspaces.reload().unwrap();
    fail_checkpoint(&app, "history_removed");
    let failed = app.handle(req("project.delete", params(&project)));
    assert_eq!(failed["ok"], false, "{failed}");
    assert_eq!(store.load_review(task).unwrap().unwrap(), saved);
    assert_eq!(pins.iter().map(|p| refs(p)).collect::<Vec<_>>(), before);
    store.writer_for_test().execute_batch("DROP TRIGGER fail_phase").unwrap();
    app.project_deletion_failure = Some("pins");
    let failed = app.handle(req("project.delete", params(&project)));
    assert_eq!(failed["ok"], false, "post-commit pin failure: {failed}");
    assert!(store.load_review(task).unwrap().is_none());
    assert_eq!(pins.iter().map(|p| refs(p)).collect::<Vec<_>>(), before);
    app.project_deletion_failure = None;
    call(&mut app, "project.delete", params(&project));
    assert!(pins.iter().all(|p| refs(p).is_empty()));
}

#[test]
fn project_deletion_inventory_does_not_block_unrelated_requests() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    finished(&mut app, &project);
    let (gate, paused) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();
    let deleting = state.clone();
    let thread = std::thread::spawn(move || {
        let (admitted, job) = deleting.lock().unwrap().dispatch_deferring("project.delete", &params(&project));
        assert!(admitted.is_ok(), "{admitted:?}");
        let mut job = job.unwrap();
        loop { let done = job.run();
            match deleting.lock().unwrap().apply_deferred_stage("project.delete", &params(&project), done) {
                crate::app::runtime::deferred::DeferredNext::Again(next) => job = next,
                crate::app::runtime::deferred::DeferredNext::Answered(result) => return result,
            }
        }
    });
    paused.wait_for_arrival();
    let acquired = state.try_lock();
    if let Ok(mut app) = acquired {
        assert!(app.handle(req("project.list", json!({})))["ok"].as_bool().unwrap());
        filed(&mut app, &sibling, "Unrelated request during inventory");
        app.off_lock_gate = None;
    } else {
        paused.release();
        let _ = thread.join();
        panic!("inventory held the app mutex");
    }
    paused.release();
    assert!(thread.join().unwrap().is_ok());
}
