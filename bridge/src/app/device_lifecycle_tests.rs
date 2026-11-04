//! Non-SQL publication boundaries use only synthetic registries and files.
use super::*;
use rusqlite::OptionalExtension;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, AppState, crate::store::Store) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    let mut app = AppState::new(repo, root.path().join("worktrees"), "main", true, "unused.sock");
    let store = crate::store::Store::new(root.path().join("store")).unwrap();
    app.store = Some(store.clone());
    (root, app, store)
}

fn row(root: &std::path::Path, kind: &str, key: &str) -> Option<serde_json::Value> {
    let db = rusqlite::Connection::open(root.join("store/build.db")).unwrap();
    let raw: Option<String> = db.query_row(
        "SELECT record FROM sync_projection WHERE kind=?1 AND key=?2 AND deleted=0",
        rusqlite::params![kind, key], |row| row.get(0)).optional().unwrap();
    raw.map(|raw| serde_json::from_str(&raw).unwrap())
}

#[test]
fn lifecycle_project_sources_publish_registry_ids_and_only_durable_fields() {
    let (root, mut app, _store) = fixture();
    app.project_set_isolation(&json!({"project_id":"proj-1","isolation":null})).unwrap();
    let project = row(root.path(), "project", "proj-1").expect("registered project must be published");
    assert_eq!(project["project_id"], "proj-1");
    let source = row(root.path(), "project_source", "proj-1/source-1").expect("source must be published");
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
    assert_eq!(std::fs::read(workspace.root.join(crate::workspace::MANIFEST_FILE)).unwrap(), before);
}

#[test]
fn lifecycle_startup_recovers_crashes_on_both_sides_of_dirty_marker() {
    for after_marker in [false, true] {
        let (root, mut app, store) = fixture();
        let workspace = app.workspaces.begin("proj-1", "before-crash", &[]).unwrap();
        let writer = store.writer();
        let generation = writer.mark_sync_dirty("workspace", &workspace.id, 1).unwrap();
        writer.reconcile_sync_dirty("workspace", &workspace.id, generation,
            Some(json!({"id":workspace.id,"name":"stale"})), 1).unwrap();
        if after_marker {
            writer.mark_sync_dirty("registry", "lifecycle", 2).unwrap();
        }
        // The manifest is authoritative even if a process died before marking,
        // as with an external edit while the daemon was stopped.
        app.workspaces.rename(&workspace.id, "after-crash").unwrap();
        drop(app);
        let repo = root.path().join("repository");
        let mut restarted = AppState::new(repo, root.path().join("worktrees"), "main", true, "unused.sock");
        restarted.store = Some(store);
        restarted.initialize_snapshot_projection();
        let recovered = row(root.path(), "workspace", &workspace.id).unwrap();
        assert_eq!(recovered["name"], "after-crash", "after_marker={after_marker}");
        assert_eq!(recovered["project_id"], "proj-1");
    }
}

#[test]
fn lifecycle_reconcile_tombstones_deleted_registry_members_without_runtime_badges() {
    let (root, mut app, _store) = fixture();
    let workspace = app.workspaces.begin("proj-1", "durable", &[]).unwrap();
    app.workspace_set_locked(&json!({"workspace_id":workspace.id,"locked":true})).unwrap();
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
    let deleted: bool = db.query_row("SELECT deleted FROM sync_projection WHERE kind='workspace' AND key=?1", [&workspace.id], |row| row.get(0)).unwrap();
    assert!(deleted, "removed registry members require retained tombstones");
}

#[test]
fn lifecycle_deferred_workspace_keeps_fence_until_authoritative_settlement() {
    let (root, mut app, store) = fixture();
    let params = json!({"project_id":"proj-1","name":"new-workspace"});
    let (reply, job) = app.dispatch_deferring("workspace.create", &params);
    reply.unwrap();
    let job = job.expect("workspace provisioning runs outside the app lock");
    assert!(matches!(store.writer().admit_device_lease("opening", "epoch", i64::MAX / 2),
        Err(crate::store::sync::SyncError::Unavailable(ref reason)) if reason == "sync_reconciling"));
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
    store.writer().admit_device_lease("opening", "epoch", i64::MAX / 2).unwrap();
}
