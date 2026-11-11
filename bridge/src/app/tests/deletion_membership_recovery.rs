//! Deletion owns recovery of a membership projection that failed after rename.
use super::tracker::{tracked, tracked_with_origin};
use super::workflow::lifecycle_offlock::frame_on_a_thread;
use super::*;

fn fail_projection(store: &crate::store::Store) {
    store.writer_for_test().execute_batch("CREATE TEMP TRIGGER fail_deletion_membership BEFORE DELETE ON secret_project_membership BEGIN SELECT RAISE(ABORT,'injected post-rename membership failure'); END;").unwrap();
}
fn heal_projection(store: &crate::store::Store) {
    store
        .writer_for_test()
        .execute_batch("DROP TRIGGER fail_deletion_membership")
        .unwrap();
}
fn membership(store: &crate::store::Store, project: &str) -> Option<bool> {
    use rusqlite::OptionalExtension;
    store
        .writer_for_test()
        .query_row(
            "SELECT archived FROM secret_project_membership WHERE project_id=?1",
            [project],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
}
fn environment_metadata(store: &crate::store::Store, project: &str) -> bool {
    let query =
        serde_json::from_value(json!({"scope":{"project_id":project},"include_environment":true}))
            .unwrap();
    store
        .writer()
        .secret_metadata_page("synthetic-device", "synthetic-user", &query)
        .is_ok()
}

#[test]
fn project_deletion_post_config_membership_failure_recovers_in_own_reconciliation() {
    for archive_sibling in [false, true] {
        post_config_membership_failure_recovers(archive_sibling);
    }
}

fn post_config_membership_failure_recovers(archive_sibling: bool) {
    let root = tempfile::tempdir().unwrap();
    let (mut app, project) = tracked_with_origin(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    finish_sibling_checkout(&mut app, &sibling);
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    let store = app.store.as_ref().unwrap().clone();
    assert!(environment_metadata(&store, &sibling));
    fail_projection(&store);
    let (gate, paused) = OffLockGate::new();
    app.deletion_reconciliation_gate = Some(gate);
    let shared = app.shared();
    let params = json!({"project_id":project,"confirm":true});
    let deleting = frame_on_a_thread(&shared, "delete", "project.delete", params.clone());
    paused.wait_for_arrival();
    // File rename landed, but SQL still contains the old membership and
    // both fences must deny its use until deletion's own refresh succeeds.
    let config: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("config.json")).unwrap()).unwrap();
    assert!(!config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == project));
    assert!(shared.lock().unwrap().project(&project).is_none());
    assert!(!store.writer().secret_membership_clean().unwrap());
    assert!(!environment_metadata(&store, &sibling));
    shared.lock().unwrap().deletion_reconciliation_gate = None;
    heal_projection(&store);
    archive_sibling_if_requested(archive_sibling, &shared, &store, &sibling);
    paused.release();
    let reply = deleting.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(
        reply["ok"], false,
        "the original post-rename failure remains explicit: {reply}"
    );
    assert_reconciled_membership(&shared, store.clone(), project, sibling, archive_sibling);
    let retry = frame_on_a_thread(&shared, "retry", "project.delete", params)
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert_eq!(retry["ok"], true, "{retry}");
    assert!(store.writer().secret_membership_clean().unwrap());
    assert!(!shared.lock().unwrap().secret_membership_dirty.get());
}

#[test]
fn project_deletion_final_membership_failure_refuses_success_and_completed_retry_repairs_it() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    let store = app.store.as_ref().unwrap().clone();
    let (gate, paused) = OffLockGate::new();
    app.deletion_reconciliation_gate = Some(gate);
    let shared = app.shared();
    let params = json!({"project_id":project,"confirm":true});
    let deleting = frame_on_a_thread(&shared, "delete", "project.delete", params.clone());
    paused.wait_for_arrival();
    assert!(shared
        .lock()
        .unwrap()
        .project_deletion_records()
        .unwrap()
        .iter()
        .any(|r| r.tombstone.as_deref() == Some("deleted")));
    fail_projection(&store);
    store
        .writer()
        .mark_sync_dirty("secrets", "membership", crate::store::sync::now_ms())
        .unwrap();
    {
        let mut app = shared.lock().unwrap();
        app.secret_membership_dirty.set(true);
        app.deletion_reconciliation_gate = None;
    }
    paused.release();
    let refused = deleting.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(
        refused["ok"], false,
        "a completed journal must not report success while membership is fenced: {refused}"
    );
    assert!(!store.writer().secret_membership_clean().unwrap());
    assert!(!environment_metadata(&store, &sibling));
    assert!(shared.lock().unwrap().secret_membership_dirty.get());
    heal_projection(&store);
    let retry = frame_on_a_thread(&shared, "retry", "project.delete", params)
        .recv_timeout(Duration::from_secs(30))
        .unwrap();
    assert_eq!(retry["ok"], true, "{retry}");
    assert!(store.writer().secret_membership_clean().unwrap());
    assert!(!shared.lock().unwrap().secret_membership_dirty.get());
    assert!(!shared.lock().unwrap().device_lifecycle_dirty.get());
    assert_eq!(membership(&store, &project), None);
    assert_eq!(membership(&store, &sibling), Some(false));
    assert!(environment_metadata(&store, &sibling));
}

fn finish_sibling_checkout(app: &mut AppState, sibling: &str) {
    // Adopt the shared primary root first, then record its actual finish
    // proof so the sibling is idle when the concurrent archive runs.
    app.refresh_workspace_snapshot_observations();
    let workspaces: Vec<_> = app
        .workspaces
        .list(Some(sibling))
        .into_iter()
        .cloned()
        .collect();
    assert!(!workspaces.is_empty());
    for mut workspace in workspaces {
        app.workspaces.record_finished(&mut workspace).unwrap();
        app.workspaces.apply_local_capabilities(workspace).unwrap();
    }
}

fn archive_sibling_if_requested(
    archive_sibling: bool,
    shared: &Arc<Mutex<AppState>>,
    store: &crate::store::Store,
    sibling: &str,
) {
    if archive_sibling {
        let archived = frame_on_a_thread(
            shared,
            "archive",
            "project.archive",
            json!({"project_id":sibling}),
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
        assert_eq!(archived["ok"], true, "{archived}");
        // Reject even an attempted stale-open projection after the newer
        // sibling config commit. Rechecking only after SQL is too late.
        store.writer_for_test().execute_batch(&format!("CREATE TEMP TRIGGER refuse_stale_membership BEFORE INSERT ON secret_project_membership WHEN NEW.project_id='{sibling}' AND NEW.archived=0 BEGIN SELECT RAISE(ABORT,'stale membership snapshot'); END;")).unwrap();
    }
}

fn assert_reconciled_membership(
    shared: &Arc<Mutex<AppState>>,
    store: crate::store::Store,
    project: String,
    sibling: String,
    archive_sibling: bool,
) {
    assert!(
        store.writer().secret_membership_clean().unwrap(),
        "deletion's own final stage did not clear the durable fence"
    );
    {
        let app = shared.lock().unwrap();
        assert!(
            !app.secret_membership_dirty.get(),
            "in-memory membership stayed fenced"
        );
        assert!(!app.device_lifecycle_dirty.get());
    }
    assert_eq!(membership(&store, &project), None);
    assert_eq!(membership(&store, &sibling), Some(archive_sibling));
    if !archive_sibling {
        assert!(environment_metadata(&store, &sibling));
    }
}
