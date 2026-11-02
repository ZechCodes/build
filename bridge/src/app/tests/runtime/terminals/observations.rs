use super::*;

fn prepared_list(
    state: &Arc<Mutex<AppState>>,
    params: Value,
) -> crate::carrier::PreparedObservation {
    state
        .lock()
        .unwrap()
        .observation_service()
        .prepare(req("term.list", params), None)
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
    let resized = call(
        &handler,
        "term.resize",
        json!({"term_id":id,"cols":111,"rows":37}),
    );
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
    for params in [
        json!({}),
        json!({"project_id":"missing"}),
        json!({"run_id":"missing"}),
        json!({"workspace_id":"missing"}),
        json!({"project_id":"missing", "worktree_id":"missing"}),
        on_source(&selector, json!({"run_id":"mixed"})),
    ] {
        let expected = crate::api::reply(
            json!("1"),
            state.lock().unwrap().route("term.list", &params, None),
        );
        let refused = prepared_list(&state, params)();
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused, expected);
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
    assert_eq!(
        refused["error"],
        crate::app::runtime::terminals::MOVED_TERMINAL_SOURCE
    );
    drop(handler);
}

#[tokio::test]
async fn terminal_observation_keeps_workspace_container_roots_and_scope_retirement() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project = state.lock().unwrap().project_at(0).id.clone();
    let created = call(
        &handler,
        "workspace.create",
        json!({
            "project_id":project, "name":"terminal container", "isolation":"worktree"
        }),
    );
    assert_eq!(created["ok"], true, "{created}");
    let workspace = created["result"]["workspace_id"].as_str().unwrap();
    let source = created["result"]["directories"][0]["source_id"]
        .as_str()
        .unwrap();
    let conversation = call(
        &handler,
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace}),
    );
    assert_eq!(conversation["ok"], true, "{conversation}");
    let run = conversation["result"]["entity_id"].as_str().unwrap();
    let workspace_scope = json!({"workspace_id":workspace,"source_id":source});
    let run_scope = json!({"run_id":run});
    let shell = call(&handler, "term.create", workspace_scope.clone());
    assert_eq!(shell["ok"], true, "{shell}");
    let id = shell["result"]["term_id"].as_str().unwrap();
    // The container has no .git; the run's Git scope resolves its child source.
    assert!(!Path::new(created["result"]["root"].as_str().unwrap())
        .join(".git")
        .exists());
    for scope in [&workspace_scope, &run_scope] {
        let listed = prepared_list(&state, scope.clone())();
        assert_eq!(listed_ids(&listed), [id], "{listed}");
    }
    let waiting_workspace = prepared_list(&state, workspace_scope.clone());
    let waiting_run = prepared_list(&state, run_scope.clone());
    let closed = call(&handler, "term.close", json!({"term_id":id}));
    assert_eq!(closed["ok"], true, "{closed}");
    let deleted = call(
        &handler,
        "workspace.delete",
        json!({"workspace_id":workspace}),
    );
    assert_eq!(deleted["ok"], true, "{deleted}");
    for refused in [
        waiting_workspace(),
        waiting_run(),
        prepared_list(&state, workspace_scope)(),
        prepared_list(&state, run_scope)(),
    ] {
        assert_eq!(refused["ok"], false, "{refused}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_observation_lists_live_shell_while_its_screen_mutex_is_held() {
    use crate::app::tests::runtime::native_reads::NativeReads;

    let mut reads = NativeReads::new();
    reads.state.lock().unwrap().term_shell = "/bin/bash".into();
    let handler = AppState::handler(Arc::clone(&reads.state));
    let project = reads.project.clone();
    let created = call(
        &handler,
        "term.create",
        json!({"project_id":project,"cols":111,"rows":37}),
    );
    assert_eq!(created["ok"], true, "{created}");
    let term_id = created["result"]["term_id"].as_str().unwrap().to_owned();
    let screen = {
        let app = reads.state.lock().unwrap();
        let key = app.tab_key_of_wire_id(&term_id).unwrap();
        app.session_registry.terminal_screen(&key).unwrap()
    };
    let (ready, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _held = screen.hold();
        ready.send(()).unwrap();
        let _ = released.recv();
    });
    arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    // Joining a separate task lets us release the actual screen lock before
    // propagating a timed-out carrier assertion from the shared helper.
    let result = tokio::spawn(async move {
        reads
            .held_read("term.list", json!({"project_id":project}))
            .await
    })
    .await;
    release.send(()).unwrap();
    holder.join().unwrap();
    let closed = call(&handler, "term.close", json!({"term_id":term_id}));
    assert_eq!(closed["ok"], true, "{closed}");
    let answer = result.expect("term.list must finish while the terminal screen mutex is held");
    assert_eq!(answer["ok"], true, "{answer}");
    let rows = answer["result"]["terminals"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{answer}");
    assert_eq!(rows[0]["term_id"], term_id);
    assert_eq!(rows[0]["cols"], 111);
    assert_eq!(rows[0]["rows"], 37);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_observation_publishes_created_shell_while_unrelated_plan_is_borrowed() {
    use crate::app::tests::runtime::native_reads::NativeReads;

    let mut reads = NativeReads::new();
    reads.state.lock().unwrap().term_shell = "/bin/bash".into();
    let handler = AppState::handler(Arc::clone(&reads.state));
    let project = reads.project.clone();
    let borrowed = {
        let mut app = reads.state.lock().unwrap();
        let borrowed = app.take_plan(&reads.task).unwrap();
        assert!(app.committed_entities.has_borrowed(&app));
        borrowed
    };
    let created = call(&handler, "term.create", json!({"project_id":project}));
    assert_eq!(created["ok"], true, "{created}");
    let id = created["result"]["term_id"].as_str().unwrap();
    let listed = reads
        .held_read("term.list", json!({"project_id":project}))
        .await;
    reads
        .state
        .lock()
        .unwrap()
        .plans
        .insert(reads.task.clone(), borrowed);
    let closed = call(&handler, "term.close", json!({"term_id":id}));
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(listed_ids(&listed), [id], "{listed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_observation_publishes_closed_shell_while_unrelated_plan_is_borrowed() {
    use crate::app::tests::runtime::native_reads::NativeReads;

    let mut reads = NativeReads::new();
    reads.state.lock().unwrap().term_shell = "/bin/bash".into();
    let handler = AppState::handler(Arc::clone(&reads.state));
    let project = reads.project.clone();
    let created = call(&handler, "term.create", json!({"project_id":project}));
    assert_eq!(created["ok"], true, "{created}");
    let id = created["result"]["term_id"].as_str().unwrap();
    let before = reads
        .held_read("term.list", json!({"project_id":project}))
        .await;
    assert_eq!(listed_ids(&before), [id], "{before}");
    let borrowed = {
        let mut app = reads.state.lock().unwrap();
        let borrowed = app.take_plan(&reads.task).unwrap();
        assert!(app.committed_entities.has_borrowed(&app));
        borrowed
    };
    let closed = call(&handler, "term.close", json!({"term_id":id}));
    assert_eq!(closed["ok"], true, "{closed}");
    let listed = reads
        .held_read("term.list", json!({"project_id":project}))
        .await;
    reads
        .state
        .lock()
        .unwrap()
        .plans
        .insert(reads.task.clone(), borrowed);
    assert!(listed_ids(&listed).is_empty(), "{listed}");
}
