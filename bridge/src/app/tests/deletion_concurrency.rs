//! Real routed requests beside cleanup; admission must never touch SQLite.
use super::environment_deletion::call;
use super::project_agent::workspace;
use super::tracker::{filed, tracked};
use super::workflow::lifecycle_offlock::frame_on_a_thread;
use super::*;

#[test]
fn project_deletion_cleanup_allows_sibling_rename_and_workspace_delete() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let sibling = app.add_environment_sibling_for_test(&project);
    let own = workspace(&mut app, &project, "Delete this");
    let other = workspace(&mut app, &sibling, "Keep independent");
    let own_root = app.workspaces.get(&own).unwrap().root.clone();
    let other_root = app.workspaces.get(&other).unwrap().root.clone();
    app.config_path = Some(root.path().join("config.json"));
    app.persist();
    let (gate, handle) = OffLockGate::new();
    app.deletion_cleanup_gate = Some(gate);
    let shared = app.shared();
    let deleting = frame_on_a_thread(
        &shared,
        "delete",
        "project.delete",
        json!({"project_id":project,"confirm":true}),
    );
    handle.wait_for_arrival();
    assert!(own_root.exists());
    let refused = frame_on_a_thread(
        &shared,
        "own",
        "project.rename",
        json!({"project_id":project,"name":"Refuse"}),
    )
    .recv_timeout(Duration::from_secs(10))
    .unwrap();
    assert_eq!(refused["ok"], false, "target stays fenced: {refused}");
    let adopting = frame_on_a_thread(
        &shared,
        "adopt",
        "project.add_source",
        json!({"project_id":sibling,"path":own_root,"name":"Cannot acquire deleting files"}),
    )
    .recv_timeout(Duration::from_secs(10))
    .unwrap();
    assert_eq!(
        adopting["ok"], false,
        "captured paths stay fenced: {adopting}"
    );
    assert!(
        adopting["error"]
            .as_str()
            .unwrap()
            .contains("deletion-pending"),
        "{adopting}"
    );
    let renamed = frame_on_a_thread(
        &shared,
        "rename",
        "project.rename",
        json!({"project_id":sibling,"name":"Sibling renamed"}),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("rename completes during paused cleanup");
    let removed = frame_on_a_thread(
        &shared,
        "workspace",
        "workspace.delete",
        json!({"workspace_id":other}),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("unrelated workspace delete completes during paused cleanup");
    let created = frame_on_a_thread(
        &shared,
        "create",
        "workspace.create",
        json!({"project_id":sibling,"name":"Created during cleanup","isolation":"worktree"}),
    )
    .recv_timeout(Duration::from_secs(10))
    .expect("unrelated creation completes during cleanup");
    handle.release();
    let result = deleting.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(renamed["ok"], true, "{renamed}");
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(created["ok"], true, "{created}");
    assert!(Path::new(created["result"]["root"].as_str().unwrap()).exists());
    assert_eq!(result["ok"], true, "{result}");
    assert!(!own_root.exists());
    assert!(!other_root.exists());
    let mut app = shared.lock().unwrap();
    assert_eq!(app.project(&sibling).unwrap().name, "Sibling renamed");
    let config: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("config.json")).unwrap()).unwrap();
    assert!(config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == sibling && p["name"] == "Sibling renamed"));
    call(&mut app, "project.list", json!({}));
}

#[test]
fn project_deletion_admission_is_sql_free_with_and_without_fences() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "Admission");
    let sibling = app.add_environment_sibling_for_test(&project);
    let other = filed(&mut app, &sibling, "Sibling");
    let store = app.store.as_ref().unwrap().clone();
    for pending in [false, true] {
        if pending {
            app.project_deletion_failure = Some("config");
            let refused = app.handle(req(
                "project.delete",
                json!({"project_id":project,"confirm":true,"tasks":"delete"}),
            ));
            assert_eq!(refused["ok"], false);
        }
        store
            .task_authority_queries
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let shared = Arc::new(Mutex::new(app));
        {
            let app = shared.lock().unwrap();
            let result = app.guard_archived_rpc("tasks.comment", &json!({"task_id":task["id"]}));
            assert_eq!(result.is_err(), pending);
            assert!(app
                .guard_archived_rpc("tasks.comment", &json!({"task_id":other["id"]}))
                .is_ok());
            assert_eq!(
                store
                    .task_authority_queries
                    .load(std::sync::atomic::Ordering::Relaxed),
                0,
                "RPC admission queried SQLite under the app lock, pending={pending}"
            );
        }
        app = Arc::try_unwrap(shared).ok().unwrap().into_inner().unwrap();
    }
}
