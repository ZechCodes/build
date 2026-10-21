use super::*;

fn prepared_list(state: &Arc<Mutex<AppState>>, params: Value) -> crate::carrier::PreparedObservation {
    state.lock().unwrap().observation_service().prepare(req("term.list", params), None)
}

#[tokio::test]
async fn terminal_observation_keeps_live_resize_and_retirement_facts() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project = state.lock().unwrap().project_at(0).id.clone();
    let params = json!({"project_id":project});
    let created = call(&handler, "term.create", params.clone());
    assert_eq!(created["ok"], true, "{created}");
    let id = created["result"]["term_id"].clone();
    let prepared = prepared_list(&state, params.clone());
    let resized = call(&handler, "term.resize", json!({"term_id":id,"cols":111,"rows":37}));
    assert_eq!(resized["ok"], true, "{resized}");
    let listed = prepared();
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["result"]["terminals"][0]["cols"], 111);
    assert_eq!(listed["result"]["terminals"][0]["rows"], 37);
    let closed = call(&handler, "term.close", json!({"term_id":id}));
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(listed_ids(&prepared_list(&state, params)()).is_empty());
}

#[tokio::test]
async fn terminal_observation_keeps_scope_refusals_and_source_fences() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project = state.lock().unwrap().project_at(0).id.clone();
    let selector = source_selector(&state, &project, "source-1");
    let good = prepared_list(&state, selector.clone())();
    assert_eq!(good["ok"], true, "{good}");
    for params in [json!({"project_id":"missing"}), json!({"run_id":"missing"}),
        json!({"workspace_id":"missing"}), json!({"project_id":"missing", "worktree_id":"missing"}), on_source(&selector, json!({"run_id":"mixed"}))] {
        let expected = state.lock().unwrap().term_list(&params).unwrap_err();
        let refused = prepared_list(&state, params)();
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused["error"], expected);
    }
    let prepared = prepared_list(&state, selector.clone());
    let moved = dir.path().join("moved-home");
    std::fs::create_dir(&moved).unwrap();
    {
        let mut app = state.lock().unwrap();
        app.projects.source_mut(&project, "source-1").unwrap().path = moved;
    }
    // Publish the same committed identity change as the source lifecycle.
    state.lock().unwrap().publish_snapshot_projection();
    let refused = prepared();
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert_eq!(refused["error"], crate::app::runtime::terminals::MOVED_TERMINAL_SOURCE);
    drop(handler);
}
