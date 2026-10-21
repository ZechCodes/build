//! Terminal creation records presence without rebuilding board attention or
//! waiting for SQLite while holding application state.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_activity_waits_for_store_without_holding_app_state() {
    let directory = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracker::tracked(directory.path());
    app.term_shell = "/bin/sh".into();
    app.initialize_snapshot_projection();
    let workspace = app.workspaces.list(Some(&project))[0].id.clone();
    let state = Arc::new(Mutex::new(app));
    state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
    let handler = AppState::handler(Arc::clone(&state));
    let (store, before_attention, before_session) = {
        let app = state.lock().unwrap();
        (
            app.store.clone().unwrap(),
            app.board.attention().attention(&workspace).cloned(),
            app.user_session(),
        )
    };
    let writer = store.writer_for_test();
    let creating = handler.clone();
    let scope = workspace.clone();
    let worker =
        std::thread::spawn(move || call(&creating, "term.create", json!({"workspace_id": scope})));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut observed = false;
    while std::time::Instant::now() < deadline {
        if state
            .try_lock()
            .is_ok_and(|app| app.shell_tab_count() == 1 && app.user_session() != before_session)
        {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Release before joining/asserting so a red run cannot strand a child or
    // the creation thread behind the deliberately held writer.
    drop(writer);
    let created = worker.join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    let (session, attention, child) = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        (
            app.user_session(),
            app.board.attention().attention(&workspace).cloned(),
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session),
        )
    };
    let persisted = store.load_user_session().unwrap();
    let closed = call(
        &handler,
        "term.close",
        json!({"term_id": created["result"]["term_id"]}),
    );
    child.end();
    assert!(child.exited_within(Duration::from_secs(2)));
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(
        observed,
        "terminal presence must publish while SQLite is unavailable, with the app mutex released"
    );
    assert_eq!(
        attention, before_attention,
        "opening a terminal does not promote board attention"
    );
    assert_eq!(persisted, Some(session));
}
