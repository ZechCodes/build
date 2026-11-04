//! Non-SQL publication boundaries use only synthetic registries and files.
use super::*;
use rusqlite::OptionalExtension;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, AppState, crate::store::Store) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    let mut app = AppState::new(
        repo,
        root.path().join("worktrees"),
        "main",
        true,
        "unused.sock",
    );
    let store = crate::store::Store::new(root.path().join("store")).unwrap();
    app.store = Some(store.clone());
    (root, app, store)
}

fn dirty_generation(root: &std::path::Path) -> i64 {
    rusqlite::Connection::open(root.join("store/build.db")).unwrap()
        .query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key='device_dirty_generation'", [], |row|row.get(0)).unwrap()
}

fn assert_registry_ready(store: &crate::store::Store) {
    assert!(store.writer().device_registry_generation().unwrap().is_none());
    store.writer().admit_device_lease("observations", "epoch", i64::MAX / 2).unwrap();
}

#[test]
fn lifecycle_observation_read_job_does_not_fence_or_write_an_unchanged_registry() {
    let (root, mut app, store) = fixture();
    app.refresh_workspace_snapshot_observations();
    let before = dirty_generation(root.path());
    app.defer_project_list();
    let read = app.take_deferred().unwrap();
    assert_eq!(app.active_deferred_filesystem_jobs, 1);
    app.refresh_workspace_snapshot_observations();
    assert_registry_ready(&store);
    assert_eq!(dirty_generation(root.path()), before, "read-only refresh must not pre-mark then clear");
    app.apply_deferred("project.list", &json!({}), read.run()).unwrap();
    assert_eq!(app.active_deferred_filesystem_jobs, 0);
}

#[test]
fn lifecycle_observation_unchanged_ticks_do_not_advance_dirty_generation() {
    let (root, mut app, store) = fixture();
    app.refresh_workspace_snapshot_observations();
    let before = dirty_generation(root.path());
    app.refresh_workspace_snapshot_observations();
    app.workspace_list(&json!({})).unwrap();
    assert_eq!(dirty_generation(root.path()), before);
    assert_registry_ready(&store);
}

#[test]
fn lifecycle_observation_branch_change_marks_before_manifest_and_publishes_then_settles() {
    let (root, mut app, store) = fixture();
    let created = app.dispatch("workspace.create", &json!({"project_id":"proj-1","name":"branch-observation"})).unwrap();
    let id = created["workspace_id"].as_str().unwrap();
    let workspace = app.workspaces.get(id).unwrap().clone();
    let directory = &workspace.directories[0];
    let repository = git2::Repository::init(&directory.path).unwrap();
    app.refresh_workspace_snapshot_observations();
    let before = dirty_generation(root.path());
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    let original = std::fs::read(&manifest).unwrap();
    repository.set_head("refs/heads/observed-branch").unwrap();
    store.fail_next_write();
    app.refresh_workspace_snapshot_observations();
    assert_eq!(std::fs::read(&manifest).unwrap(), original, "a marker failure must prevent manifest writes");
    app.refresh_workspace_snapshot_observations();
    assert!(dirty_generation(root.path()) > before);
    assert_eq!(row(root.path(), "workspace_directory", &format!("{id}/{}", directory.id)).unwrap()["branch"], "observed-branch");
    assert_registry_ready(&store);
}

#[test]
fn lifecycle_observation_registry_job_settles_while_unrelated_read_remains_taken() {
    let (_root, mut app, store) = fixture();
    app.refresh_workspace_snapshot_observations();
    app.defer_project_list();
    let read = app.take_deferred().unwrap();
    let params = json!({"project_id":"proj-1","name":"registry-job"});
    let (answer, job) = app.dispatch_deferring("workspace.create", &params);
    answer.unwrap();
    let job = job.unwrap();
    assert!(store.writer().device_registry_generation().unwrap().is_some());
    assert!(store.writer().admit_device_lease("observations", "epoch", i64::MAX / 2).is_err());
    assert!(matches!(app.apply_deferred_stage("workspace.create", &params, job.run()), DeferredNext::Answered(Ok(_))));
    assert_eq!(app.active_deferred_filesystem_jobs, 1);
    assert_registry_ready(&store);
    app.apply_deferred("project.list", &json!({}), read.run()).unwrap();
    assert_eq!(app.active_deferred_filesystem_jobs, 0);
}

fn row(root: &std::path::Path, kind: &str, key: &str) -> Option<serde_json::Value> {
    let db = rusqlite::Connection::open(root.join("store/build.db")).unwrap();
    let raw: Option<String> = db
        .query_row(
            "SELECT record FROM sync_projection WHERE kind=?1 AND key=?2 AND deleted=0",
            rusqlite::params![kind, key],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    raw.map(|raw| serde_json::from_str(&raw).unwrap())
}

#[test]
fn lifecycle_project_sources_publish_registry_ids_and_only_durable_fields() {
    let (root, mut app, _store) = fixture();
    app.project_set_isolation(&json!({"project_id":"proj-1","isolation":null}))
        .unwrap();
    let project =
        row(root.path(), "project", "proj-1").expect("registered project must be published");
    assert_eq!(project["project_id"], "proj-1");
    let source =
        row(root.path(), "project_source", "proj-1/source-1").expect("source must be published");
    assert_eq!(source["project_id"], "proj-1");
    for field in ["working", "pending", "attention", "remote", "sync"] {
        assert!(project.get(field).is_none(), "runtime field {field} leaked");
        assert!(source.get(field).is_none(), "observed field {field} leaked");
    }
}

#[test]
fn lifecycle_marker_failure_refuses_workspace_change_before_manifest_or_memory() {
    let (_root, mut app, store) = fixture();
    let workspace = app.workspaces.begin("proj-1", "fixture", &[]).unwrap();
    let before = std::fs::read(workspace.root.join(crate::workspace::MANIFEST_FILE)).unwrap();
    store.fail_next_write();
    let result = app.workspace_set_locked(&json!({"workspace_id":workspace.id,"locked":true}));
    assert!(result.is_err(), "marker failure must fail the action");
    assert!(result.unwrap_err().contains("sync"));
    assert!(!app.workspaces.get(&workspace.id).unwrap().locked);
    assert_eq!(
        std::fs::read(workspace.root.join(crate::workspace::MANIFEST_FILE)).unwrap(),
        before
    );
}

#[test]
fn lifecycle_startup_recovers_crashes_on_both_sides_of_dirty_marker() {
    for after_marker in [false, true] {
        let (root, mut app, store) = fixture();
        let workspace = app.workspaces.begin("proj-1", "before-crash", &[]).unwrap();
        let writer = store.writer();
        let generation = writer
            .mark_sync_dirty("workspace", &workspace.id, 1)
            .unwrap();
        writer
            .reconcile_sync_dirty(
                "workspace",
                &workspace.id,
                generation,
                Some(json!({"id":workspace.id,"name":"stale"})),
                1,
            )
            .unwrap();
        if after_marker {
            writer.mark_sync_dirty("registry", "lifecycle", 2).unwrap();
        }
        // The manifest is authoritative even if a process died before marking,
        // as with an external edit while the daemon was stopped.
        app.workspaces.rename(&workspace.id, "after-crash").unwrap();
        drop(app);
        let repo = root.path().join("repository");
        let mut restarted = AppState::new(
            repo,
            root.path().join("worktrees"),
            "main",
            true,
            "unused.sock",
        );
        restarted.store = Some(store);
        restarted.initialize_snapshot_projection();
        let recovered = row(root.path(), "workspace", &workspace.id).unwrap();
        assert_eq!(
            recovered["name"], "after-crash",
            "after_marker={after_marker}"
        );
        assert_eq!(recovered["project_id"], "proj-1");
    }
}

#[test]
fn lifecycle_reconcile_tombstones_deleted_registry_members_without_runtime_badges() {
    let (root, mut app, _store) = fixture();
    let workspace = app.workspaces.begin("proj-1", "durable", &[]).unwrap();
    app.workspace_set_locked(&json!({"workspace_id":workspace.id,"locked":true}))
        .unwrap();
    assert!(row(root.path(), "workspace", &workspace.id).is_some());
    let badge = row(root.path(), "badge", "device").expect("durable directory badge");
    for field in ["working", "running", "online", "pending"] {
        assert!(badge.get(field).is_none(), "runtime badge {field} leaked");
    }
    std::fs::remove_dir_all(&workspace.root).unwrap();
    app.workspaces.reload().unwrap();
    app.note_board_lists_changed(crate::changes::BoardLists::WORKSPACES);
    assert!(row(root.path(), "workspace", &workspace.id).is_none());
    let db = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    let deleted: bool = db
        .query_row(
            "SELECT deleted FROM sync_projection WHERE kind='workspace' AND key=?1",
            [&workspace.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        deleted,
        "removed registry members require retained tombstones"
    );
}

#[test]
fn lifecycle_deferred_workspace_keeps_fence_until_authoritative_settlement() {
    let (root, mut app, store) = fixture();
    let params = json!({"project_id":"proj-1","name":"new-workspace"});
    let (reply, job) = app.dispatch_deferring("workspace.create", &params);
    reply.unwrap();
    let job = job.expect("workspace provisioning runs outside the app lock");
    assert!(
        matches!(store.writer().admit_device_lease("opening", "epoch", i64::MAX / 2),
        Err(crate::store::sync::SyncError::Unavailable(ref reason)) if reason == "sync_reconciling")
    );
    let result = match app.apply_deferred_stage("workspace.create", &params, job.run()) {
        DeferredNext::Answered(answer) => answer.unwrap(),
        DeferredNext::Again(_) => panic!("plain directory should need one provisioning stage"),
    };
    let workspace_id = result["workspace_id"].as_str().unwrap();
    assert!(row(root.path(), "workspace", workspace_id).is_some());
    let workspace = app.workspaces.get(workspace_id).unwrap();
    for directory in &workspace.directories {
        let key = format!("{workspace_id}/{}", directory.id);
        let directory_row = row(root.path(), "workspace_directory", &key).unwrap();
        assert_eq!(directory_row["workspace_id"], workspace_id);
        assert_eq!(directory_row["project_id"], "proj-1");
    }
    // Negative control: a completed manifest publication re-opens admission.
    store
        .writer()
        .admit_device_lease("opening", "epoch", i64::MAX / 2)
        .unwrap();
}

#[test]
fn device_followup_settings_do_not_leave_the_registry_dirty() {
    let (_root, mut app, store) = fixture();
    app.settings_set(&json!({"watch_agent_filed_tasks":false}))
        .unwrap();
    store
        .writer()
        .admit_device_lease("opening", "epoch", i64::MAX / 2)
        .expect("successful settings persistence must settle its marker");
}

#[test]
fn device_followup_prepared_delivery_holds_the_fence_before_job_accounting() {
    let (_root, mut app, store) = fixture();
    let reserved = app
        .workspaces
        .reserve_delivery_workspace(
            "proj-1",
            "deferred",
            &[],
            crate::isolation::Isolation::default(),
        )
        .unwrap();
    let _job = app.prepare_deferred_workspace_snapshot(&reserved).unwrap();
    assert!(
        matches!(store.writer().admit_device_lease("opening","epoch",i64::MAX/2),Err(crate::store::sync::SyncError::Unavailable(ref reason)) if reason=="sync_reconciling")
    );
}

#[test]
fn device_followup_reopen_publishes_and_marker_failure_preserves_the_manifest() {
    let (root, mut app, store) = fixture();
    let mut workspace = app.workspaces.begin("proj-1", "finished", &[]).unwrap();
    workspace.status = crate::workspace::WorkspaceStatus::Finished;
    workspace.archived_at = Some("2026-01-01T00:00:00Z".into());
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    std::fs::write(&manifest, serde_json::to_vec(&workspace).unwrap()).unwrap();
    app.workspaces.reload().unwrap();
    app.initialize_device_lifecycle().unwrap();
    store.fail_next_write();
    assert!(app.reopen_workspace(&workspace.id).is_err());
    assert_eq!(
        app.workspaces.get(&workspace.id).unwrap().status,
        crate::workspace::WorkspaceStatus::Finished
    );
    app.reopen_workspace(&workspace.id).unwrap();
    assert_eq!(
        row(root.path(), "workspace", &workspace.id).unwrap()["status"],
        "ready"
    );
}

#[test]
fn device_followup_background_capability_refresh_publishes_without_a_list_note() {
    let (root, mut app, _store) = fixture();
    let result = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"plain"}),
        )
        .unwrap();
    let workspace = app
        .workspaces
        .get(result["workspace_id"].as_str().unwrap())
        .unwrap()
        .clone();
    let directory = workspace.directories.first().unwrap();
    git2::Repository::init(&directory.path).unwrap();
    app.refresh_workspace_snapshot_observations();
    assert_eq!(
        row(
            root.path(),
            "workspace_directory",
            &format!("{}/{}", workspace.id, directory.id)
        )
        .unwrap()["is_git"],
        true
    );
}

#[test]
fn device_followup_long_workspace_names_do_not_poison_device_recovery() {
    let (root, mut app, store) = fixture();
    let name = "workspace".repeat(20_000);
    let workspace = app.workspaces.begin("proj-1", &name, &[]).unwrap();
    app.workspace_set_locked(&json!({"workspace_id":workspace.id,"locked":true}))
        .unwrap();
    let summary = row(root.path(), "workspace", &workspace.id)
        .expect("an oversized display label still has a bounded summary");
    assert!(summary["name"].as_str().unwrap().chars().count() <= 256);
    assert_eq!(app.workspaces.get(&workspace.id).unwrap().name, name);
    store
        .writer()
        .admit_device_lease("opening", "epoch", i64::MAX / 2)
        .unwrap();
}

#[test]
fn device_followup_agent_workspace_id_uses_the_registry_directory_identity() {
    let (root, mut app, store) = fixture();
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"directory-id"}),
        )
        .unwrap();
    let workspace_id = created["workspace_id"].as_str().unwrap();
    let conversation = app
        .workspace_ensure_conversation(&json!({"workspace_id":workspace_id}))
        .unwrap();
    let owner = conversation["entity_id"].as_str().unwrap();
    let mut active = app.take_run(owner).unwrap();
    active.agents =
        crate::agent::AgentRoster::with_first(owner, Default::default(), "2026-01-01T00:00:00Z");
    app.finish_run_mutation(owner.to_owned(), active).unwrap();

    let agent_id: String = store
        .writer_for_test()
        .query_row("SELECT id FROM agents WHERE owner_id=?1", [owner], |row| {
            row.get(0)
        })
        .unwrap();
    let agent = row(root.path(), "agent", &agent_id).unwrap();
    assert_eq!(
        agent["workspace_id"], workspace_id,
        "a conversation owner id is distinct from its workspace id"
    );
    assert_eq!(agent["project_id"], "proj-1");
}

#[tokio::test]
async fn device_followup_startup_marker_failure_cannot_expose_stale_device_rows() {
    let (_root, mut app, store) = fixture();
    let generation = store
        .writer()
        .mark_sync_dirty("project", "stale", 1)
        .unwrap();
    store
        .writer()
        .reconcile_sync_dirty(
            "project",
            "stale",
            generation,
            Some(json!({"id":"stale"})),
            1,
        )
        .unwrap();
    store.fail_next_write();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    let service = app.read_service();
    let connection = store.read_pool().acquire_sync_connection().await;
    let frame = crate::transport::Frame {
        session_id: "synthetic".into(),
        message_id: "one".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        payload: json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"device"}}}),
    };
    let response = service.call_for_opening(frame, Some(connection), "opening", "epoch");
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["retryable"], true, "{response}");
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .contains("sync_reconciling"),
        "{response}"
    );
}

#[test]
fn device_followup_workspace_get_cannot_refresh_after_marker_failure() {
    let (_root, mut app, store) = fixture();
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"get-refresh"}),
        )
        .unwrap();
    let workspace_id = created["workspace_id"].as_str().unwrap();
    let workspace = app.workspaces.get(workspace_id).unwrap().clone();
    git2::Repository::init(&workspace.directories[0].path).unwrap();
    store.fail_next_write();
    let _answer = app.workspace_get(&json!({"workspace_id":workspace_id}));
    assert!(
        !app.workspaces.get(workspace_id).unwrap().directories[0].is_git,
        "a failed pre-effect marker must prevent capability refresh too"
    );
    app.workspace_get(&json!({"workspace_id":workspace_id}))
        .unwrap();
    assert!(app.workspaces.get(workspace_id).unwrap().directories[0].is_git);
}

#[test]
fn device_followup_adopted_owner_marks_before_staging_its_workspace_alias() {
    let (_root, mut app, store) = fixture();
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"adoption-marker"}),
        )
        .unwrap();
    let workspace_id = created["workspace_id"].as_str().unwrap();
    let conversation = app
        .workspace_ensure_conversation(&json!({"workspace_id":workspace_id}))
        .unwrap();
    let owner = conversation["entity_id"].as_str().unwrap().to_owned();
    let active = app.take_run(&owner).unwrap();
    store.fail_next_write();
    let error = app
        .finish_adopted_run_mutation(owner.clone(), active)
        .unwrap_err();
    assert!(
        error.contains("sync marker"),
        "adoption must fail at its marker boundary: {error}"
    );
    assert!(
        app.workspaces.get(&owner).is_none(),
        "failed marker exposed the compatibility alias"
    );
    assert!(
        app.runs.contains_key(&owner),
        "the committed owner must remain available"
    );
}
