//! Deletion recovery and the explicitly accepted older-binary residuals.
use super::environment_deletion::call;
use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::*;

// This uses the unchanged pre-journal production reader to model a rollback.
fn rollback_pending_fixture(root: &Path) -> (tempfile::TempDir, AppState, String, String, Value) {
    let (repo, mut app, project) = tracked(root);
    let task = filed(&mut app, &project, "Pending deletion history");
    let path = app.projects.get(&project).unwrap().home_path.clone();
    assert_eq!(
        app.projects.get(&project).unwrap().project_key,
        path.display().to_string()
    );
    let checkout = workspace(&mut app, &project, "Pending deletion checkout");
    let mut finished = app.workspaces.get(&checkout).unwrap().clone();
    finished.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&finished).unwrap();
    let store = app.store.as_ref().unwrap();
    let admission = crate::store::MutationAdmission {
        principal: "project-deletion".into(),
        operation_id: uuid::Uuid::new_v4().to_string(),
        method: "project.delete".into(),
        params: json!({"project_id":project,"confirm":true,"tasks":"delete"}),
        scope: json!({"project_id":project,"project_key":path}),
        owners: vec![crate::store::MutationOwner {
            kind: "project".into(),
            id: project.clone(),
            generation: 1,
        }],
        resources: vec![],
        reservations: vec![],
        after: None,
        recovery_policy: "external".into(),
        accepted_epoch: "compatibility-probe".into(),
        acceptance: json!({"cleanup":{"roots":[finished.root]},"phase":"config_removed"}),
    };
    let record = store.writer().admit_mutation(&admission).unwrap();
    store.writer_for_test().execute(
        "UPDATE mutation_commands SET state='deletion_pending',record=json_set(record,'$.state','deletion_pending') WHERE command_id=?1",
        [record.command_id],
    ).unwrap();
    let config_path = root.join("config.json");
    let mut config = app.config_value(&app.projects_dir, app.default_harness, app.isolation);
    config["projects"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["id"] != project);
    std::fs::write(&config_path, config.to_string()).unwrap();
    drop(app);
    let mut old = super::project_agent::rooted(root)
        .with_config(config_path)
        .unwrap()
        .with_task_store(root.join("store"))
        .unwrap();
    old.store
        .as_ref()
        .unwrap()
        .writer()
        .recover_mutations()
        .unwrap();
    assert!(call(&mut old, "project.list", json!({}))["projects"]
        .as_array()
        .unwrap()
        .is_empty());
    let integrity: String = old
        .store
        .as_ref()
        .unwrap()
        .writer_for_test()
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    (repo, old, project, checkout, task)
}

#[test]
fn rollback_pending_delete_old_reader_lists_orphan_checkouts() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut old, _project, checkout, _task) = rollback_pending_fixture(root.path());
    let listed = call(&mut old, "workspace.list", json!({}));
    assert!(
        listed["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == checkout),
        "accepted older-reader residual missing: {listed}"
    );
}

#[test]
fn rollback_pending_delete_old_reader_reattaches_legacy_history() {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut old, _project, _checkout, task) = rollback_pending_fixture(root.path());
    let project = super::project_agent::added_project(&mut old, &repo.path().join("repo"));
    let listed = call(&mut old, "tasks.list", json!({"project_id":project}));
    assert!(
        listed["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == task["id"]),
        "accepted older-reader residual missing: {listed}"
    );
}

#[test]
fn project_deletion_journal_failure_restart_and_retry() {
    for phase in ["record", "config", "checkout", "history"] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project) = tracked(root.path());
        let sibling = app.add_environment_sibling_for_test(&project);
        let own = filed(&mut app, &project, "Pending history");
        let kept = filed(&mut app, &sibling, "Sibling history");
        let checkout = workspace(&mut app, &project, "Pending checkout");
        let mut finished = app.workspaces.get(&checkout).unwrap().clone();
        finished.status = crate::workspace::WorkspaceStatus::Finished;
        crate::workspace::persist_review_workspace(&finished).unwrap();
        app.workspaces.reload().unwrap();
        let config = root.path().join("config.json");
        app.config_path = Some(config.clone());
        app.persist();
        let before = std::fs::read(&config).unwrap();
        app.project_deletion_failure = Some(phase);
        let params = json!({"project_id":project,"confirm":true,"tasks":"delete"});
        let failed = app.handle(req("project.delete", params.clone()));
        assert_eq!(failed["ok"], false, "phase={phase}: {failed}");
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(own["id"].as_str().unwrap())
            .unwrap()
            .is_some());
        if matches!(phase, "record" | "config") {
            assert_eq!(std::fs::read(&config).unwrap(), before);
            assert!(app.project(&project).is_some());
            assert!(finished.root.exists());
        } else {
            assert!(app.project(&project).is_none());
            assert_ne!(std::fs::read(&config).unwrap(), before);
        }
        drop(app);
        let mut app = super::project_agent::rooted(root.path())
            .with_config(&config)
            .unwrap()
            .with_task_store(root.path().join("store"))
            .unwrap();
        if phase != "record" {
            let refused = app.handle(req(
                "tasks.create",
                json!({"project_id":project,"title":"Fenced"}),
            ));
            assert_eq!(refused["ok"], false, "{refused}");
            assert!(
                refused["error"]
                    .as_str()
                    .unwrap()
                    .contains("deletion-pending"),
                "{refused}"
            );
        }
        call(&mut app, "project.delete", params.clone());
        assert!(app.project(&project).is_none());
        assert!(!finished.root.exists());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(own["id"].as_str().unwrap())
            .unwrap()
            .is_none());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(kept["id"].as_str().unwrap())
            .unwrap()
            .is_some());
        drop(app);
        let mut app = super::project_agent::rooted(root.path())
            .with_config(&config)
            .unwrap()
            .with_task_store(root.path().join("store"))
            .unwrap();
        call(&mut app, "project.delete", params);
    }
}

fn pending(
    root: &Path,
    phase: &'static str,
) -> (
    tempfile::TempDir,
    AppState,
    String,
    Value,
    crate::workspace::Workspace,
) {
    let (repo, mut app, project) = tracked(root);
    let task = filed(&mut app, &project, "Pending");
    let id = workspace(&mut app, &project, "Recorded checkout");
    let mut workspace = app.workspaces.get(&id).unwrap().clone();
    workspace.status = crate::workspace::WorkspaceStatus::Finished;
    crate::workspace::persist_review_workspace(&workspace).unwrap();
    app.workspaces.reload().unwrap();
    app.config_path = Some(root.join("config.json"));
    app.persist();
    app.project_deletion_failure = Some(phase);
    let result = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(result["ok"], false, "{result}");
    (repo, app, project, task, workspace)
}

fn restart(root: &Path) -> AppState {
    super::project_agent::rooted(root)
        .with_config(root.join("config.json"))
        .unwrap()
        .with_task_store(root.join("store"))
        .unwrap()
}

#[test]
fn project_deletion_fences_rpc_mcp_and_registration_after_config_removal() {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut app, project, task, workspace) = pending(root.path(), "checkout");
    app.project_deletion_failure = None;
    for (method, params) in [
        (
            "tasks.create",
            json!({"project_id":project,"title":"Refused"}),
        ),
        (
            "workspace.create",
            json!({"project_id":project,"name":"Refused"}),
        ),
        (
            "tasks.comment",
            json!({"task_id":task["id"],"body":"Refused"}),
        ),
        (
            "tasks.assign",
            json!({"task_id":task["id"],"assignee":{"kind":"user"}}),
        ),
        ("tasks.get", json!({"task_id":task["id"]})),
        ("project.restore", json!({"project_id":project})),
    ] {
        let refused = app.handle(req(method, params));
        assert_eq!(refused["ok"], false, "{method}: {refused}");
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("deletion-pending"),
            "{method}: {refused}"
        );
    }
    let listed = call(&mut app, "workspace.list", json!({}));
    assert!(!listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == workspace.id));
    let environment = app.handle(req("environment.delete", json!({"environment_id":"env-1"})));
    assert_eq!(environment["error"], "Delete its 1 projects first");
    let key = app.project_deletion_fences[0].project_key.clone();
    assert!(app
        .refuse_deleting_identity(&project, "project:other")
        .is_err());
    assert!(app.refuse_deleting_identity("proj-999", &key).is_err());
    assert!(app
        .create_environment_project("env-1", "Refused", &key, "proj-999", vec![], |_| Ok(()))
        .is_err());
    assert!(app
        .create_environment_project(
            "env-1",
            "Refused",
            "project:other",
            &project,
            vec![],
            |_| Ok(())
        )
        .is_err());
    let refused = app.handle(req(
        "project.create",
        open_folder(json!({"path":repo.path().join("repo")})),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(app
        .guard_archived_action(&project, &crate::mcp::BridgeAction::ListWorkspaces)
        .unwrap_err()
        .contains("deletion-pending"));
    assert!(app
        .guard_archived_action(&workspace.id, &crate::mcp::BridgeAction::ListWorkspaces)
        .is_err());
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert!(app.project_deletion_fences.is_empty());
}

#[test]
fn project_deletion_mismatched_identity_and_mode_leave_state_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project, task, _workspace) = pending(root.path(), "config");
    let bytes = std::fs::read(root.path().join("config.json")).unwrap();
    let record = serde_json::to_value(app.project_deletion_records().unwrap()).unwrap();
    app.projects.at_mut(0).project_key = "project:mismatched".into();
    let refused = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("identity mismatch"),
        "{refused}"
    );
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        bytes
    );
    assert_eq!(
        serde_json::to_value(app.project_deletion_records().unwrap()).unwrap(),
        record
    );
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    app.projects.at_mut(0).project_key = app.project_deletion_fences[0].project_key.clone();
    let refused = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        serde_json::to_value(app.project_deletion_records().unwrap()).unwrap(),
        record
    );
    app.project_deletion_failure = None;
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
}

// Hide the new kind from the current reader to exercise exactly the old
// reader's policy: unknown states are ignored and no deletion fence exists.
fn enter_old_reader(app: &mut AppState) {
    app.store.as_ref().unwrap().writer_for_test().execute(
        "UPDATE mutation_commands SET principal='compatibility-old-reader' WHERE principal='internal/project-deletion'",[]).unwrap();
    app.project_deletion_fences.clear();
}
fn leave_old_reader(app: &AppState) {
    app.store.as_ref().unwrap().writer_for_test().execute(
        "UPDATE mutation_commands SET principal='internal/project-deletion' WHERE principal='compatibility-old-reader'",[]).unwrap();
}

#[test]
fn project_deletion_return_trip_abandons_readded_registration() {
    let root = tempfile::tempdir().unwrap();
    let (repo, mut old, original, task, workspace) = pending(root.path(), "checkout");
    enter_old_reader(&mut old);
    let readded = super::project_agent::added_project(&mut old, &repo.path().join("repo"));
    assert_ne!(readded, original);
    let new_task = filed(&mut old, &readded, "Written under older binary");
    leave_old_reader(&old);
    drop(old);
    let mut app = restart(root.path());
    assert!(app.project_deletion_fences.is_empty());
    assert!(workspace.root.exists());
    assert!(app.project(&readded).is_some());
    assert_eq!(
        app.workspaces.get(&workspace.id).unwrap().project_id,
        original
    );
    assert!(app
        .project_deletion_records()
        .unwrap()
        .iter()
        .any(|r| r.tombstone.as_deref() == Some("abandoned")));
    call(
        &mut app,
        "tasks.comment",
        json!({"task_id":task["id"],"body":"Usable after abandonment"}),
    );
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(new_task["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    // The original address is no longer authority to execute a stale plan.
    let refused = app.handle(req(
        "project.delete",
        json!({"project_id":original,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(workspace.root.exists());
    call(
        &mut app,
        "project.delete",
        json!({"project_id":readded,"confirm":true,"tasks":"delete"}),
    );
    assert!(!workspace.root.exists());
    assert!(app.workspaces.get(&workspace.id).is_none());
}

#[test]
fn project_deletion_return_trip_abandons_changed_history_and_workspace() {
    for change in ["history", "workspace"] {
        let root = tempfile::tempdir().unwrap();
        let (repo, mut old, project, task, workspace) = pending(root.path(), "checkout");
        enter_old_reader(&mut old);
        if change == "history" {
            let readded = super::project_agent::added_project(&mut old, &repo.path().join("repo"));
            call(
                &mut old,
                "tasks.comment",
                json!({"task_id":task["id"],"body":"Preserve old-binary change"}),
            );
            old.projects.remove(&readded);
            old.persist();
            call(
                &mut old,
                "environment.delete",
                json!({"environment_id":"env-1"}),
            );
        } else {
            std::fs::write(workspace.root.join("old-binary-work"), "preserve this").unwrap();
        }
        leave_old_reader(&old);
        drop(old);
        let mut app = restart(root.path());
        assert!(app.project_deletion_fences.is_empty(), "{change}");
        assert!(app.project(&project).is_some(), "{change}");
        assert!(workspace.root.exists());
        call(
            &mut app,
            "tasks.comment",
            json!({"task_id":task["id"],"body":"Still usable"}),
        );
        call(
            &mut app,
            "project.delete",
            json!({"project_id":project,"confirm":true,"tasks":"delete"}),
        );
        assert!(!workspace.root.exists());
    }
}

#[test]
fn environment_delete_post_success_restart_retry_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let config = root.path().join("config.json");
    app.config_path = Some(config.clone());
    app.persist();
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    let bytes = std::fs::read(&config).unwrap();
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    drop(app);
    let mut app = restart(root.path());
    call(
        &mut app,
        "environment.delete",
        json!({"environment_id":"env-1"}),
    );
    assert_eq!(std::fs::read(&config).unwrap(), bytes);
}

#[test]
fn project_deletion_history_and_phase_commit_atomically() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project, task, _workspace) = pending(root.path(), "history");
    app.project_deletion_failure = None;
    // Test-only failure after the task DELETE statements, at the very same
    // transaction's checkpoint. Nothing from that transaction may survive.
    app.store
        .as_ref()
        .unwrap()
        .writer_for_test()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_deletion_checkpoint BEFORE UPDATE ON mutation_commands
         WHEN json_extract(NEW.record,'$.execution.phase')='history_removed'
         BEGIN SELECT RAISE(ABORT,'injected history checkpoint failure'); END;",
        )
        .unwrap();
    let params = json!({"project_id":project,"confirm":true,"tasks":"delete"});
    let failed = app.handle(req("project.delete", params.clone()));
    assert_eq!(failed["ok"], false, "{failed}");
    let record = app.project_deletion_records().unwrap().pop().unwrap();
    assert_eq!(record.phase, "files_removed");
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_some());
    let bytes = std::fs::read(root.path().join("config.json")).unwrap();
    assert!(app.project(&project).is_none());
    drop(app); // TEMP trigger disappears at actual database close.
    let mut app = restart(root.path());
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        bytes
    );
    call(&mut app, "project.delete", params);
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_tracker_task(task["id"].as_str().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn project_deletion_record_write_failure_leaves_project_usable() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    let before = std::fs::read(root.path().join("config.json")).unwrap();
    app.store
        .as_ref()
        .unwrap()
        .writer_for_test()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_deletion_admission BEFORE INSERT ON mutation_commands
         WHEN NEW.principal='internal/project-deletion'
         BEGIN SELECT RAISE(ABORT,'injected deletion record write failure'); END;",
        )
        .unwrap();
    let result = app.handle(req(
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    ));
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(
        std::fs::read(root.path().join("config.json")).unwrap(),
        before
    );
    assert!(app.project_deletion_fences.is_empty());
    assert!(app.project_deletion_records().unwrap().is_empty());
    filed(
        &mut app,
        &project,
        "Still usable after refused record write",
    );
    drop(app);
    let mut app = restart(root.path());
    filed(&mut app, &project, "Still usable after restart");
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
}

#[test]
fn project_deletion_real_config_write_and_rename_failures_are_retryable() {
    for step in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project, task, workspace) = pending(root.path(), "record");
        app.project_deletion_failure = None;
        app.config_persist_failure = Some(step);
        let bytes = std::fs::read(root.path().join("config.json")).unwrap();
        let params = json!({"project_id":project,"confirm":true,"tasks":"delete"});
        let failed = app.handle(req("project.delete", params.clone()));
        assert_eq!(failed["ok"], false, "{failed}");
        assert_eq!(
            std::fs::read(root.path().join("config.json")).unwrap(),
            bytes
        );
        assert!(app.project(&project).is_some());
        assert!(workspace.root.exists());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(task["id"].as_str().unwrap())
            .unwrap()
            .is_some());
        drop(app);
        let mut app = restart(root.path());
        assert!(!app.project_deletion_fences.is_empty());
        call(&mut app, "project.delete", params);
    }
}

#[tokio::test]
async fn project_deletion_fences_independent_sql_and_mcp_task_reads() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project, task, _workspace) = pending(root.path(), "checkout");
    app.initialize_read_context();
    let store = app.store.as_ref().unwrap();
    let reply = app.read_service().call(
        req("tasks.get", json!({"task_id":task["id"]})),
        Some(store.read_pool().acquire().await),
    );
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"]
            .as_str()
            .unwrap()
            .contains("deletion-pending"),
        "{reply}"
    );
    let mut connection = store.read_pool().acquire().await;
    let refusal = app
        .read_service()
        .mcp_tasks(
            &project,
            &crate::app::read_context::mcp_tasks::TaskQuery::Get {
                task_id: task["id"].as_str().unwrap().into(),
            },
            Some(&mut connection),
        )
        .unwrap_err();
    assert!(refusal.contains("deletion-pending"), "{refusal}");
}

#[test]
fn project_deletion_retry_prunes_captured_checkout_name_after_manifest_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project, _task, workspace) = pending(root.path(), "checkout");
    let record = app.project_deletion_records().unwrap().pop().unwrap();
    let checkout = &record.plan.checkouts[0];
    let source = checkout.source.clone();
    let name = checkout.registered_name.clone();
    std::fs::remove_dir_all(&workspace.root).unwrap();
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&name)
        .is_ok());
    app.project_deletion_failure = None;
    drop(app);
    let mut app = restart(root.path());
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert!(git2::Repository::open(source)
        .unwrap()
        .find_worktree(&name)
        .is_err());
}

#[test]
fn project_deletion_return_trip_abandons_retargeted_checkout_registration() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, app, project, _task, workspace) = pending(root.path(), "checkout");
    let record = app.project_deletion_records().unwrap().pop().unwrap();
    let checkout = &record.plan.checkouts[0];
    let moved = root.path().join("moved-by-user");
    git_in(
        &checkout.source,
        &[
            "worktree",
            "move",
            checkout.path.to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    drop(app);
    let mut app = restart(root.path());
    assert!(app.project_deletion_fences.is_empty());
    assert!(app.project(&project).is_some());
    assert!(moved.join("README.md").exists());
    call(
        &mut app,
        "project.delete",
        json!({"project_id":project,"confirm":true,"tasks":"delete"}),
    );
    assert!(moved.join("README.md").exists());
    assert!(!workspace.root.exists());
}

#[test]
fn project_deletion_checkpoint_failures_restart_and_retry() {
    for phase in ["config_removed", "files_removed", "complete"] {
        let root = tempfile::tempdir().unwrap();
        let (_repo, mut app, project, task, workspace) = pending(root.path(), "record");
        app.project_deletion_failure = None;
        app.store
            .as_ref()
            .unwrap()
            .writer_for_test()
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER fail_phase_checkpoint BEFORE UPDATE ON mutation_commands
             WHEN json_extract(NEW.record,'$.execution.phase')='{phase}'
             BEGIN SELECT RAISE(ABORT,'injected phase checkpoint failure'); END;"
            ))
            .unwrap();
        let params = json!({"project_id":project,"confirm":true,"tasks":"delete"});
        let failed = app.handle(req("project.delete", params.clone()));
        assert_eq!(failed["ok"], false, "phase={phase}: {failed}");
        assert!(app.project(&project).is_none());
        let remains = app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(task["id"].as_str().unwrap())
            .unwrap()
            .is_some();
        assert_eq!(remains, phase != "complete");
        let record = app.project_deletion_records().unwrap().pop().unwrap();
        assert!(record.tombstone.is_none());
        drop(app);
        let mut app = restart(root.path());
        assert!(!app.project_deletion_fences.is_empty(), "phase={phase}");
        call(&mut app, "project.delete", params);
        assert!(!workspace.root.exists());
        assert!(app.project_deletion_fences.is_empty());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_tracker_task(task["id"].as_str().unwrap())
            .unwrap()
            .is_none());
    }
}
