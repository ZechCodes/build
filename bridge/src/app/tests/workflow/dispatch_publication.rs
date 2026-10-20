use super::*;

#[test]
fn branch_dispatch_publishes_its_workspace_before_its_reply() {
    let (home, repo) = init_repo();
    let mut app = qa_state(&repo, home.path());
    let project = app.project_at(0).id.clone();
    let snapshots = app.snapshot_service();

    let dispatched = app.handle(req(
        "branch.dispatch",
        json!({"project_id": project, "instruction": "Publish the workspace"}),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched}");
    let owner = dispatched["result"]["run_id"].as_str().unwrap();
    let agent = dispatched["result"]["agent_id"].as_str().unwrap();
    let workspace = snapshots.call(req(
        "workspace.get",
        json!({"workspace_id": owner, "agent_id": agent}),
    ));
    assert_eq!(workspace["ok"], true, "{workspace}");
    assert_eq!(workspace["result"]["run_id"], owner, "{workspace}");
    assert_eq!(
        workspace["result"]["run"]["agents"][0]["id"], agent,
        "{workspace}"
    );
    assert!(
        serde_json::to_string(&workspace)
            .unwrap()
            .contains("Publish the workspace"),
        "the alias carries the dispatched instruction: {workspace}"
    );
}

#[test]
fn failed_branch_dispatch_save_leaves_no_workspace_alias_after_an_unrelated_write() {
    let (home, repo) = init_repo();
    let mut app = qa_state(&repo, home.path());
    let project = app.project_at(0).id.clone();
    let (gate, gate_handle) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();
    let dispatched = frame_on_a_thread(
        &state,
        "dispatch-alias-save-failure",
        "branch.dispatch",
        json!({"project_id": project, "instruction": "Refuse this workspace"}),
    );
    gate_handle.wait_for_arrival();
    let (owner, marker) = {
        let app = state.lock().unwrap();
        let owner = app.pending_rows[0].entity_id.clone();
        let marker = app.workspaces.root().join(".locks").join(&owner);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, b"locked").unwrap();
        (owner, marker)
    };
    let connection = rusqlite::Connection::open(home.path().join("store/build.db")).unwrap();
    connection
        .execute_batch("CREATE TRIGGER refuse_dispatch_run BEFORE INSERT ON implementations BEGIN SELECT RAISE(ABORT, 'dispatch-save-refused'); END")
        .unwrap();
    gate_handle.release();
    let failed = dispatched
        .recv_timeout(Duration::from_secs(30))
        .expect("the refused dispatch answers");
    assert_eq!(failed["ok"], false, "{failed}");
    assert!(
        failed["error"]
            .as_str()
            .unwrap()
            .contains("dispatch-save-refused"),
        "{failed}"
    );
    connection
        .execute_batch("DROP TRIGGER refuse_dispatch_run")
        .unwrap();

    let mut app = state.lock().unwrap();
    assert!(!app.runs.contains_key(&owner));
    assert!(app.workspaces.get(&owner).is_none());
    assert!(app.delivery_queue.queued_is_empty());
    file_legacy_task(&mut app, "unrelated accepted write");
    app.publish_snapshot_projection();
    assert!(app.workspaces.get(&owner).is_none());
    assert!(marker.is_file(), "rollback preserves the checkout marker");
    let snapshots = app.snapshot_service();
    let workspace = snapshots.call(req("workspace.get", json!({"workspace_id": owner})));
    assert_eq!(workspace["ok"], false, "{workspace}");
    let listed = snapshots.call(req("workspace.list", json!({})));
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["workspace_id"] != owner));
}
