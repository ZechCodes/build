//! Non-SQL publication boundaries use only synthetic registries and files.
use super::*;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

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
    rusqlite::Connection::open(root.join("store/build.db"))
        .unwrap()
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM meta WHERE key='device_dirty_generation'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn observation_workspace(app: &mut AppState, name: &str) -> crate::workspace::Workspace {
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":name}),
        )
        .unwrap();
    let workspace = app
        .workspaces
        .get(created["workspace_id"].as_str().unwrap())
        .unwrap()
        .clone();
    let repository = git2::Repository::init(&workspace.directories[0].path).unwrap();
    repository.set_head("refs/heads/before").unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
    let tree_id = repository.index().unwrap().write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let commit_id = repository
        .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
        .unwrap();
    repository
        .branch("after", &repository.find_commit(commit_id).unwrap(), false)
        .unwrap();
    workspace
}

fn block_workspace_manifest(workspace: &crate::workspace::Workspace) -> std::path::PathBuf {
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    let backup = workspace.root.join("manifest.saved");
    std::fs::rename(&manifest, &backup).unwrap();
    std::fs::create_dir(&manifest).unwrap();
    backup
}
fn restore_workspace_manifest(workspace: &crate::workspace::Workspace, backup: &std::path::Path) {
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    std::fs::remove_dir(&manifest).unwrap();
    std::fs::rename(backup, manifest).unwrap();
}

#[tokio::test]
async fn lifecycle_observation_failed_manifest_does_not_starve_later_workspace_or_fence_reads() {
    let (root, mut app, store) = fixture();
    let pair = [
        observation_workspace(&mut app, "first"),
        observation_workspace(&mut app, "second"),
    ];
    app.refresh_workspace_snapshot_observations();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    for workspace in &pair {
        git2::Repository::open(&workspace.directories[0].path)
            .unwrap()
            .set_head("refs/heads/after")
            .unwrap();
    }
    // Select the first candidate from the actual pass, independent of HashMap order.
    let observations = app.workspaces.observe_local_capabilities();
    assert_eq!(observations.len(), 2);
    let failed = &observations[0];
    let healthy = &observations[1];
    let backup = block_workspace_manifest(failed);
    let failed_key = format!("{}/{}", failed.id, failed.directories[0].id);
    let healthy_key = format!("{}/{}", healthy.id, healthy.directories[0].id);
    for _ in 0..3 {
        app.refresh_workspace_snapshot_observations();
        assert_eq!(
            row(root.path(), "workspace_directory", &healthy_key).unwrap()["branch"],
            "after",
            "a failed first manifest must not starve the later workspace's branch publication"
        );
        assert!(
            store
                .writer()
                .device_registry_generation()
                .unwrap()
                .is_none(),
            "a partial refresh must settle"
        );
        let page = lease_page(&app, &store, "failed-manifest", None, 500).await;
        let failed_row = page["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["kind"] == "workspace_directory" && record["key"] == failed_key)
            .unwrap();
        assert_eq!(
            failed_row["row"]["branch"], "before",
            "reads keep the failed workspace's last committed observation"
        );
        assert_eq!(
            app.workspaces.get(&failed.id).unwrap().directories[0]
                .branch
                .as_deref(),
            Some("before")
        );
    }
    restore_workspace_manifest(failed, &backup);
    app.refresh_workspace_snapshot_observations();
    assert_eq!(
        row(root.path(), "workspace_directory", &failed_key).unwrap()["branch"],
        "after",
        "the uncommitted observation is retried after the manifest recovers"
    );
    assert_registry_ready(&store);
}

#[tokio::test]
async fn lifecycle_observation_failed_reopen_does_not_starve_later_workspace_or_fence_reads() {
    let (root, mut app, store) = fixture();
    let pair = [
        observation_workspace(&mut app, "first-finished"),
        observation_workspace(&mut app, "second-finished"),
    ];
    app.refresh_workspace_snapshot_observations();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    for original in &pair {
        let mut workspace = app.workspaces.get(&original.id).unwrap().clone();
        workspace.status = crate::workspace::WorkspaceStatus::Finished;
        workspace.archived_at = Some("2026-01-01T00:00:00Z".into());
        workspace.directories[0].finished_head = None;
        std::fs::write(
            workspace.root.join(crate::workspace::MANIFEST_FILE),
            serde_json::to_vec(&workspace).unwrap(),
        )
        .unwrap();
    }
    app.workspaces.reload().unwrap();
    // Startup itself refreshes workspaces, so install stale proofs afterwards.
    app.initialize_device_lifecycle().unwrap();
    let observations = app.workspaces.observe_finished_local();
    assert_eq!(observations.len(), 2);
    let failed = app.workspaces.get(&observations[0]).unwrap().clone();
    let healthy = app.workspaces.get(&observations[1]).unwrap().clone();
    let backup = block_workspace_manifest(&failed);
    for _ in 0..3 {
        app.refresh_workspace_snapshot_observations();
        assert_eq!(
            row(root.path(), "workspace", &healthy.id).unwrap()["status"],
            "ready",
            "a failed first reopen must not starve the later workspace"
        );
        assert!(store
            .writer()
            .device_registry_generation()
            .unwrap()
            .is_none());
        let page = lease_page(&app, &store, "failed-reopen", None, 500).await;
        let failed_row = page["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["kind"] == "workspace" && record["key"] == failed.id)
            .unwrap();
        assert_eq!(
            failed_row["row"]["status"], "finished",
            "failed reopen must preserve committed status in reads"
        );
    }
    restore_workspace_manifest(&failed, &backup);
    app.refresh_workspace_snapshot_observations();
    assert_eq!(
        row(root.path(), "workspace", &failed.id).unwrap()["status"],
        "ready"
    );
    assert_registry_ready(&store);
}

fn assert_registry_ready(store: &crate::store::Store) {
    assert!(store
        .writer()
        .device_registry_generation()
        .unwrap()
        .is_none());
    store
        .writer()
        .admit_device_lease("observations", "epoch", i64::MAX / 2)
        .unwrap();
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
    assert_eq!(
        dirty_generation(root.path()),
        before,
        "read-only refresh must not pre-mark then clear"
    );
    app.apply_deferred("project.list", &json!({}), read.run())
        .unwrap();
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
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"branch-observation"}),
        )
        .unwrap();
    let id = created["workspace_id"].as_str().unwrap();
    let workspace = app.workspaces.get(id).unwrap().clone();
    let directory = &workspace.directories[0];
    let repository = git2::Repository::init(&directory.path).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.test").unwrap();
    let tree_id = repository.index().unwrap().write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let commit_id = repository
        .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
        .unwrap();
    repository
        .branch(
            "observed-branch",
            &repository.find_commit(commit_id).unwrap(),
            false,
        )
        .unwrap();
    app.refresh_workspace_snapshot_observations();
    let before = dirty_generation(root.path());
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    let original = std::fs::read(&manifest).unwrap();
    repository.set_head("refs/heads/observed-branch").unwrap();
    store.fail_next_write();
    app.refresh_workspace_snapshot_observations();
    assert_eq!(
        std::fs::read(&manifest).unwrap(),
        original,
        "a marker failure must prevent manifest writes"
    );
    app.refresh_workspace_snapshot_observations();
    assert!(dirty_generation(root.path()) > before);
    assert_eq!(
        row(
            root.path(),
            "workspace_directory",
            &format!("{id}/{}", directory.id)
        )
        .unwrap()["branch"],
        "observed-branch"
    );
    assert_registry_ready(&store);
}

#[test]
fn lifecycle_observation_unrelated_git_job_does_not_hold_an_existing_marker() {
    let (_root, mut app, store) = fixture();
    app.refresh_workspace_snapshot_observations();
    app.defer_git_work(UnrelatedGitFixture, |_, _| Ok(json!({})), &json!({}), true);
    let job = app.take_deferred().unwrap();
    app.mark_device_lifecycle_dirty().unwrap();
    app.settle_device_lifecycle();
    assert_registry_ready(&store);
    app.apply_deferred("fs.fixture", &json!({}), job.run())
        .unwrap();
    assert_eq!(app.active_deferred_filesystem_jobs, 0);
    app.defer_watch(crate::app::runtime::deferred::WatchAnswer::Unsubscribed);
    let watch = app.take_deferred().unwrap();
    app.mark_device_lifecycle_dirty().unwrap();
    app.settle_device_lifecycle();
    assert_registry_ready(&store);
    app.apply_deferred("changes.unsubscribe", &json!({}), watch.run())
        .unwrap();
}

struct UnrelatedGitFixture;
impl GitCallScope for UnrelatedGitFixture {
    fn invalidate(&self, _: &mut AppState) {}
}

struct RegistryGitFixture;
impl GitCallScope for RegistryGitFixture {
    fn changes_device_registry(&self) -> bool {
        true
    }
    fn invalidate(&self, _: &mut AppState) {}
}
struct RefusedRegistryHandoff;
impl crate::app::git::deferred::DeferredGitWork for RefusedRegistryHandoff {
    fn run(&self, _: &Value) -> Result<Value, String> {
        Ok(json!({}))
    }
    fn invalidate(&self, _: &mut AppState) {}
    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        app.defer_git_work(RegistryGitFixture, |_, _| Ok(json!({})), &json!({}), true);
        Err("synthetic refused registry handoff".into())
    }
}

#[test]
fn device_fix_round_refused_registry_handoff_does_not_strand_its_fence() {
    let (_root, mut app, store) = fixture();
    app.refresh_workspace_snapshot_observations();
    app.mark_device_lifecycle_dirty().unwrap();
    app.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
        call: Box::new(RefusedRegistryHandoff),
        params: Value::Null,
        invalidates: false,
        registry_change: true,
        gate: None,
    })));
    let job = app.take_deferred().unwrap();
    assert!(matches!(
        app.apply_deferred_stage("registry.fixture", &json!({}), job.run()),
        DeferredNext::Answered(Err(_))
    ));
    assert!(app.deferred_work.is_none());
    assert_eq!(app.active_device_registry_jobs, 0);
    assert_eq!(app.active_deferred_filesystem_jobs, 0);
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
    assert!(store
        .writer()
        .device_registry_generation()
        .unwrap()
        .is_some());
    assert!(store
        .writer()
        .admit_device_lease("observations", "epoch", i64::MAX / 2)
        .is_err());
    assert!(matches!(
        app.apply_deferred_stage("workspace.create", &params, job.run()),
        DeferredNext::Answered(Ok(_))
    ));
    assert_eq!(app.active_deferred_filesystem_jobs, 1);
    assert_registry_ready(&store);
    app.apply_deferred("project.list", &json!({}), read.run())
        .unwrap();
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

fn snapshot_frame(page: Option<&str>, limit: usize) -> crate::transport::Frame {
    crate::transport::Frame {
        session_id: "lease-test".into(),
        message_id: "snapshot".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "now".into(),
        payload: json!({"id":1,"method":"sync.snapshot","params":{"scope":{"kind":"device"},"page":page,"limit":limit}}),
    }
}
async fn lease_page(
    app: &AppState,
    store: &crate::store::Store,
    opening: &str,
    page: Option<&str>,
    limit: usize,
) -> Value {
    let connection = store.read_pool().acquire_sync_connection().await;
    let answer = app.read_service().call_for_opening(
        snapshot_frame(page, limit),
        Some(connection),
        opening,
        "epoch",
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["status"], "ok", "{answer}");
    answer["result"].clone()
}
fn lease_count(store: &crate::store::Store) -> i64 {
    store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_leases", [], |row| row.get(0))
        .unwrap()
}
fn retained_bytes(store: &crate::store::Store) -> i64 {
    store
        .writer_for_test()
        .query_row("SELECT retained_version_bytes FROM sync_state", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[tokio::test]
async fn device_fix_round_five_complete_snapshots_and_seventeen_openings_release_quota() {
    let (_root, mut app, store) = fixture();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    for _ in 0..5 {
        let answer = lease_page(&app, &store, "one-opening", None, 500).await;
        assert!(answer.get("next_page").is_none());
    }
    for index in 0..17 {
        lease_page(&app, &store, &format!("opening-{index}"), None, 500).await;
    }
    assert_eq!(lease_count(&store), 0);
}

#[tokio::test]
async fn device_fix_round_completed_snapshot_releases_retained_versions_toward_head() {
    let (_root, mut app, store) = fixture();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    store
        .writer()
        .sync_transaction(|_, batch| {
            batch.upsert(
                crate::store::sync::Entity::new("lease_fixture", "one"),
                json!({"name":"before"}),
                &[crate::store::sync::Stream::device()],
            );
            Ok(())
        })
        .unwrap();
    let first = lease_page(&app, &store, "gc-opening", None, 1).await;
    let page = first["next_page"].as_str().unwrap();
    store
        .writer()
        .sync_transaction(|_, batch| {
            batch.upsert(
                crate::store::sync::Entity::new("lease_fixture", "one"),
                json!({"name":"after"}),
                &[crate::store::sync::Stream::device()],
            );
            Ok(())
        })
        .unwrap();
    store
        .writer()
        .sync_gc_batch(crate::store::sync::now_ms())
        .unwrap();
    assert!(
        retained_bytes(&store) > 0,
        "an unfinished snapshot must protect old versions"
    );
    let last = lease_page(&app, &store, "gc-opening", Some(page), 500).await;
    assert!(last.get("next_page").is_none());
    store
        .writer()
        .sync_gc_batch(crate::store::sync::now_ms())
        .unwrap();
    assert_eq!(
        retained_bytes(&store),
        0,
        "finished snapshot must stop pinning the cleanup floor"
    );
    assert_eq!(lease_count(&store), 0);
}

#[tokio::test]
async fn device_fix_round_busy_writer_serves_final_page_and_leaves_lease_to_expire() {
    let (_root, mut app, store) = fixture();
    app.initialize_snapshot_projection();
    app.initialize_read_context();
    let first = lease_page(&app, &store, "busy-opening", None, 1).await;
    let page = first["next_page"].as_str().unwrap().to_owned();
    let service = app.read_service();
    let connection = store.read_pool().acquire_sync_connection().await;
    let held = store.writer_for_test();
    let (sent, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        sent.send(service.call_for_opening(
            snapshot_frame(Some(&page), 500),
            Some(connection),
            "busy-opening",
            "epoch",
        ))
        .unwrap();
    });
    let answer = received.recv_timeout(std::time::Duration::from_millis(500));
    drop(held);
    reader.join().unwrap();
    let answer = answer.expect("lease cleanup must not wait for the held writer");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["status"], "ok");
    assert!(answer["result"].get("next_page").is_none());
    assert_eq!(lease_count(&store), 1, "busy cleanup is best effort");
    store
        .writer()
        .sync_gc_batch(first["lease"]["expires_at_ms"].as_i64().unwrap() + 1)
        .unwrap();
    assert_eq!(lease_count(&store), 0);
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

#[test]
fn device_334_sessions_equal_workspace_and_project_list_snapshots_for_unwatched_activity() {
    let (root, mut app, store) = fixture();
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"pooled-session-334"}),
        )
        .unwrap();
    let workspace_id = created["workspace_id"].as_str().unwrap();
    let conversation = app
        .workspace_ensure_conversation(&json!({"workspace_id":workspace_id}))
        .unwrap();
    let owner = conversation["entity_id"].as_str().unwrap().to_owned();
    for at in [
        "2026-01-01T00:00:00Z",
        "2026-01-01T07:00:00Z",
        "2026-01-02T02:00:00Z",
    ] {
        let mut active = app.take_run(&owner).unwrap();
        if active.agents.is_empty() {
            active.agents = crate::agent::AgentRoster::with_first(
                &owner,
                Default::default(),
                "2026-01-01T00:00:00Z",
            );
        }
        let agent = active.agents.primary_mut().unwrap();
        agent.watched = false;
        if at == "2026-01-02T02:00:00Z" {
            agent
                .thread
                .post_agent_progress("unwatched progress session fact", None, at);
        } else {
            agent.thread.post_agent("unwatched session fact", None, at);
        }
        app.finish_run_mutation(owner.clone(), active).unwrap();
        let workspace = app.workspaces.get(workspace_id).unwrap();
        let workspace_row = app.workspace_list_snapshot_row(workspace).unwrap();
        let project_row = app.project_list()["projects"][0].clone();
        for (kind, key, list) in [
            ("owner_session", owner.as_str(), workspace_row),
            ("project_session", "proj-1", project_row),
        ] {
            let device = row(root.path(), kind, key).expect("durable session record");
            for field in ["session_started_ms", "last_activity_ms"] {
                assert_eq!(device[field], list[field], "{kind}.{field} at {at}");
            }
        }
    }
    let mut active = app.take_run(&owner).unwrap();
    let agent = active.agents.primary_mut().unwrap();
    let agent_id = agent.id.clone();
    for _ in 0..150 {
        agent
            .thread
            .post_agent("reclaimable session history", None, "2026-01-02T03:00:00Z");
    }
    app.finish_run_mutation(owner.clone(), active).unwrap();
    let expected = row(root.path(), "owner_session", &owner).unwrap();
    let reclaimed = store
        .prune_conversation_batch(
            &agent_id,
            crate::store::RetentionPolicy::default(),
            "2026-11-24T12:00:00Z",
        )
        .unwrap();
    assert!(reclaimed.items_deleted > 0);
    app.rebuild_session_summaries().unwrap();
    let workspace = app
        .workspace_list_snapshot_row(app.workspaces.get(workspace_id).unwrap())
        .unwrap();
    for field in ["session_started_ms", "last_activity_ms"] {
        assert_eq!(
            workspace[field], expected[field],
            "durable boundary after prefix reclamation: {field}"
        );
    }
}

#[test]
fn device_334_sessions_unmapped_startup_keeps_exact_pooled_list_fields() {
    let (root, mut app, store) = fixture();
    let created = app
        .dispatch(
            "workspace.create",
            &json!({"project_id":"proj-1","name":"unmapped-session-334"}),
        )
        .unwrap();
    let conversation = app
        .workspace_ensure_conversation(&json!({"workspace_id":created["workspace_id"]}))
        .unwrap();
    let owner = conversation["entity_id"].as_str().unwrap().to_owned();
    let mut active = app.take_run(&owner).unwrap();
    active.agents =
        crate::agent::AgentRoster::with_first(&owner, Default::default(), "2026-01-01T00:00:00Z");
    active.agents.primary_mut().unwrap().thread.post_user(
        "live activity",
        None,
        "2026-01-01T07:00:00Z",
    );
    app.finish_run_mutation(owner, active).unwrap();
    let first = crate::session_summary::message_millis("2026-01-01T00:00:00Z").unwrap();
    let last = crate::session_summary::message_millis("2026-01-01T07:00:00Z").unwrap();
    store
        .writer_for_test()
        .execute(
            "INSERT INTO inbox_retained_messages VALUES('proj-1',?1)",
            [first],
        )
        .unwrap();
    store.writer_for_test().execute_batch("DELETE FROM device_project_ids; DELETE FROM device_session_project_ids; DELETE FROM device_pooled_sessions; DELETE FROM meta WHERE key='device_sessions_334_position';").unwrap();
    app.store = None;
    drop(store);
    let store = crate::store::Store::new(root.path().join("store")).unwrap();
    app.store = Some(store);
    app.rebuild_session_summaries().unwrap();
    let project = app.project_list()["projects"][0].clone();
    assert_eq!(project["session_started_ms"], first);
    assert_eq!(project["last_activity_ms"], last);
    app.initialize_device_lifecycle().unwrap();
    let device = row(root.path(), "project_session", "proj-1").unwrap();
    for field in ["session_started_ms", "last_activity_ms"] {
        assert_eq!(device[field], project[field], "startup mapping: {field}");
    }
}

fn secret_fence_fixture() -> (tempfile::TempDir, AppState, crate::store::Store) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().to_path_buf(),
    )
    .unwrap();
    let mut app =
        AppState::new_configured(repo, root.path().join("worktrees"), "main", true, context);
    let store = crate::store::Store::new(root.path().join("store")).unwrap();
    app.store = Some(store.clone());
    (root, app, store)
}

fn secrets_refused_refresh_cycles(cycles: usize) {
    let (_root, app, store) = secret_fence_fixture();
    let shared = app.with_change_window(std::time::Duration::ZERO).shared();
    let bus = shared.lock().unwrap().changes();
    let (sender, mut rx, key) = crate::carrier::SessionSender::observable("fenced-secrets");
    bus.subscribe(
        &sender,
        crate::changes::SubscriptionSpec {
            id: "secrets".into(),
            scope: crate::changes::Scope::Entity("proj-1".into()),
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    shared
        .lock()
        .unwrap()
        .initialize_device_lifecycle()
        .unwrap();
    // Join commits, then the off-lock phase queues its hint without delivering.
    bus.publish_pending_secret_updates();
    let query: crate::api::v1::secrets::ListParams =
        serde_json::from_value(json!({"scope":{"project_id":"proj-1"}})).unwrap();
    let audit: crate::api::v1::secrets::AuditParams = serde_json::from_value(json!({"scope":{"project_id":"proj-1"},"secret_id":"sec_00000000-0000-4000-8000-000000000001"})).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pool = store.read_pool();
    let mut reader = runtime.block_on(pool.acquire());
    for _ in 0..cycles {
        shared
            .lock()
            .unwrap()
            .mark_device_lifecycle_dirty()
            .unwrap();
        assert!(bus.flush() > 0);
        let hint = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
        assert_eq!(hint["scope"], "project");
        assert!(matches!(
            store
                .writer()
                .secret_metadata_page("device", "device_user", &query),
            Err(crate::secrets::SecretError::Unavailable)
        ));
        assert!(matches!(
            store
                .writer()
                .secret_metadata_audit("device", "device_user", &audit),
            Err(crate::secrets::SecretError::Unavailable)
        ));
        assert!(matches!(
            reader
                .read(|r| r.secret_metadata_page("device", "device_user", &query))
                .unwrap(),
            Err(crate::secrets::SecretError::Unavailable)
        ));
        shared.lock().unwrap().settle_device_lifecycle();
        // Membership is identical. The refused scope still queues a replacement.
        bus.publish_pending_secret_updates();
    }
    assert!(
        bus.flush() > 0,
        "each dirty interval owes a post-clear retry"
    );
    let retry = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(retry["truncated"], true);
    assert!(retry.get("environment_id").is_none());
    assert!(store
        .writer()
        .secret_metadata_page("device", "device_user", &query)
        .is_ok());
    assert!(reader
        .read(|r| r.secret_metadata_page("device", "device_user", &query))
        .unwrap()
        .is_ok());
}

#[test]
fn secrets_hinted_read_refused_by_unrelated_dirty_write_gets_post_clear_retry() {
    secrets_refused_refresh_cycles(1);
}
#[test]
fn secrets_hinted_read_refused_in_two_consecutive_dirty_intervals_gets_each_retry() {
    secrets_refused_refresh_cycles(2);
}
#[test]
fn secrets_membership_publication_waits_for_app_unlock_and_fence_clear() {
    let (_root, app, store) = secret_fence_fixture();
    // Share outside a Tokio context; only the explicit push flusher runs.
    let shared = app.with_change_window(std::time::Duration::ZERO).shared();
    let bus = shared.lock().unwrap().changes();
    let (sender, mut rx, key) = crate::carrier::SessionSender::observable("unlock-secrets");
    bus.subscribe(
        &sender,
        crate::changes::SubscriptionSpec {
            id: "secrets".into(),
            scope: crate::changes::Scope::All,
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    let push = crate::liveness::DedicatedRuntime::push().unwrap();
    crate::changes::ChangeBus::spawn_flusher_on(bus.clone(), Some(push.handle()));
    // Completion barriers, not elapsed-time assertions or scheduling sleeps.
    let settle = || {
        push.handle().block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(20), bus.settle_for_test())
                .await
                .expect("flusher completion barrier")
        })
    };
    let guard = shared.lock().unwrap();
    guard.mark_device_lifecycle_dirty().unwrap();
    settle();
    assert!(rx.try_recv().is_err());
    drop(guard);
    settle();
    assert!(rx.try_recv().is_err(), "unlock alone is not a commit");
    let guard = shared.lock().unwrap();
    guard.settle_device_lifecycle();
    settle();
    assert!(
        rx.try_recv().is_err(),
        "SQL clear alone must not enqueue under app"
    );
    drop(guard);
    settle();
    let event = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(event["scope"], "project");
    let query = serde_json::from_value(json!({"scope":{"project_id":"proj-1"}})).unwrap();
    shared
        .lock()
        .unwrap()
        .mark_device_lifecycle_dirty()
        .unwrap();
    assert!(store
        .writer()
        .secret_metadata_page("device", "device_user", &query)
        .is_err());
    settle();
    assert!(rx.try_recv().is_err());
    shared.lock().unwrap().settle_device_lifecycle();
    settle();
    let retry = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
    assert_eq!(retry["truncated"], true);
    assert!(store
        .writer()
        .secret_metadata_page("device", "device_user", &query)
        .is_ok());
}

#[test]
fn secrets_late_dirty_snapshot_refusal_after_drain_rearms_all_without_unbounded_keys() {
    let (_root, app, store) = secret_fence_fixture();
    let shared = app.with_change_window(std::time::Duration::ZERO).shared();
    let bus = shared.lock().unwrap().changes();
    shared
        .lock()
        .unwrap()
        .initialize_device_lifecycle()
        .unwrap();
    bus.flush();
    let (sender, mut rx, key) = crate::carrier::SessionSender::observable("late-secret-read");
    bus.subscribe(
        &sender,
        crate::changes::SubscriptionSpec {
            id: "secrets".into(),
            scope: crate::changes::Scope::All,
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pool = store.read_pool();
    let mut reader = runtime.block_on(pool.acquire());
    shared
        .lock()
        .unwrap()
        .mark_device_lifecycle_dirty()
        .unwrap();
    reader
        .read(|read| {
            for index in 0..100 {
                let query = serde_json::from_value(
                    json!({"scope":{"project_id":format!("unknown-{index}")}}),
                )
                .unwrap();
                assert!(matches!(
                    read.secret_metadata_page("device", "device_user", &query),
                    Err(crate::secrets::SecretError::Unavailable)
                ));
            }
            assert_eq!(store.writer().secret_read_retries.pending_count(), 1);
            shared.lock().unwrap().settle_device_lifecycle();
            assert!(bus.flush() > 0);
            let event = crate::carrier::SessionSender::decrypt_push(&key, &rx.try_recv().unwrap());
            assert_eq!(event["scope"], "device");
            assert_eq!(store.writer().secret_read_retries.pending_count(), 0);
            // The same independent SQL snapshot still sees dirty after the drain.
            let query = serde_json::from_value(json!({"scope":{"project_id":"proj-1"}})).unwrap();
            assert!(matches!(
                read.secret_metadata_page("device", "device_user", &query),
                Err(crate::secrets::SecretError::Unavailable)
            ));
        })
        .unwrap();
    assert!(bus.flush() > 0, "late refusal must wake another capture");
    assert!(rx.try_recv().is_ok());
    let query = serde_json::from_value(json!({"scope":{"project_id":"proj-1"}})).unwrap();
    assert!(reader
        .read(|r| r.secret_metadata_page("device", "device_user", &query))
        .unwrap()
        .is_ok());
}

#[test]
fn secrets_real_settings_write_rehints_refused_read_after_unchanged_membership_settles() {
    let (root, app, store) = secret_fence_fixture();
    let path = root.path().join("config.json");
    let app = app.with_config(&path).unwrap();
    let requested = !app.watch_agent_filed_tasks;
    app.persist();
    let before: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let shared = app.with_change_window(std::time::Duration::ZERO).shared();
    let bus = shared.lock().unwrap().changes();
    let (sender, mut rx, key) = crate::carrier::SessionSender::observable("real-settings-secrets");
    bus.subscribe(
        &sender,
        crate::changes::SubscriptionSpec {
            id: "secrets".into(),
            scope: crate::changes::Scope::Entity("proj-1".into()),
            kinds: [crate::changes::Kind::SecretsChanged].into_iter().collect(),
            mode: Default::default(),
            priority: Default::default(),
        },
    );
    shared
        .lock()
        .unwrap()
        .initialize_device_lifecycle()
        .unwrap();
    bus.publish_pending_secret_updates();
    let (gate, control) = crate::test_support::off_lock::OffLockGate::new();
    shared.lock().unwrap().config_persist_gate = Some(gate);
    let writer_state = shared.clone();
    let write = std::thread::spawn(move || {
        writer_state.lock().unwrap().dispatch(
            "settings.set",
            &json!({"watch_agent_filed_tasks":requested}),
        )
    });
    control.wait_for_arrival();
    // The actual settings persistence path must have marked the SQL fence.
    let dirty_during_write = !store.writer().secret_registry_clean().unwrap();
    let push = crate::liveness::DedicatedRuntime::push().unwrap();
    crate::changes::ChangeBus::spawn_flusher_on(bus.clone(), Some(push.handle()));
    let join_hint = push.handle().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .expect("queued join hint is delivered")
            .expect("subscription stays open")
    });
    let query = serde_json::from_value(json!({"scope":{"project_id":"proj-1"}})).unwrap();
    let refused = matches!(
        store
            .writer()
            .secret_metadata_page("device", "device_user", &query),
        Err(crate::secrets::SecretError::Unavailable)
    );
    // Always release/join before assertions, including the negative control.
    control.release();
    let result = write.join().unwrap().unwrap();
    assert!(
        dirty_during_write,
        "settings.set must mark the registry fence"
    );
    let hint = crate::carrier::SessionSender::decrypt_push(&key, &join_hint);
    assert_eq!(hint["scope"], "project");
    assert!(refused, "the hinted read overlaps the real settings write");
    assert_eq!(result["watch_agent_filed_tasks"], requested);
    let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["watch_agent_filed_tasks"], requested);
    assert_ne!(
        before["watch_agent_filed_tasks"],
        after["watch_agent_filed_tasks"]
    );
    assert_eq!(before["projects"], after["projects"]);
    assert_eq!(before["environments"], after["environments"]);
    assert!(
        store.writer().secret_registry_clean().unwrap(),
        "settings.set must settle its fence"
    );
    // No manual post-write flush: normal settlement must drive the replacement.
    let replacement = push.handle().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
            .await
            .expect("settings settlement automatically re-hints the refused scope")
            .expect("subscription stays open")
    });
    let retry = crate::carrier::SessionSender::decrypt_push(&key, &replacement);
    assert_eq!(retry["scope"], "project");
    assert_eq!(retry["project_id"], "proj-1");
    assert_eq!(retry["truncated"], true);
    assert!(store
        .writer()
        .secret_metadata_page("device", "device_user", &query)
        .is_ok());
}
