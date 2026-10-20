use super::*;

fn save_message(app: &mut AppState, owner: &str, goal: &str, body: &str) -> (String, u64) {
    let mut active = app.take_run(owner).unwrap();
    active.run.goal = goal.to_owned();
    let thread = active.agents.sole_thread_mut();
    thread.post_agent(body, None, "2099-01-01T00:00:00Z");
    let tip = (thread.id.clone(), thread.last_sequence());
    app.finish_run_mutation(owner.to_owned(), active).unwrap();
    tip
}

fn routed(handler: &FrameHandler, method: &str, params: Value) -> Value {
    let answer = handler.call(
        SessionSender::detached("review-reader"),
        req(method, params),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

fn assert_published(
    handler: &FrameHandler,
    pushes: &[Value],
    owner: &str,
    goal: &str,
    body: &str,
    tip: u64,
) {
    let board = routed(handler, "board.list", json!({}));
    let agents = routed(handler, "agent.list", json!({"entity_id": owner}));
    let changed = push::changed_entities(pushes);
    let evidence = json!({"board": board, "agents": agents, "pushes": pushes});
    assert!(
        serde_json::to_string(&board).unwrap().contains(goal)
            && agents["agents"][0]["unread_count"] == tip
            && changed.contains(&owner.to_owned())
            && serde_json::to_string(pushes).unwrap().contains(body),
        "acknowledged owner changes must reach routed reads and their subscribed notices: {evidence}"
    );
}

#[tokio::test]
async fn merge_release_keeps_later_reads_notices_and_news_live() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut incoming, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (released, survivor) = {
        let mut app = state.lock().unwrap();
        let released = adopted_run(&mut app, &repo, dir.path(), "released-owner");
        let survivor = adopted_run(&mut app, &repo, dir.path(), "surviving-owner");
        assert!(app.surviving_workspace_of_run(&released).is_none());
        (released, survivor)
    };
    push::settled_pushes(&mut incoming, &key).await;
    let control_tip = save_message(
        &mut state.lock().unwrap(),
        &survivor,
        "before release",
        "before release news",
    );
    let control = push::settled_pushes(&mut incoming, &key).await;
    assert_published(
        &handler,
        &control,
        &survivor,
        "before release",
        "before release news",
        control_tip.1,
    );

    let tip = {
        let mut app = state.lock().unwrap();
        let project = app.project_of(&released).unwrap();
        let worktree = app.runs[&released].worktree.clone();
        app.apply_merge_cleanup(&released, &project, &worktree, MergeCleanup::Release);
        assert!(!app.runs.contains_key(&released));
        assert!(
            worktree.path.exists(),
            "release keeps the external checkout"
        );
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .iter()
            .all(|record| record.id != released));
        let tip = save_message(&mut app, &survivor, "after release", "after release news");
        let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
        assert_eq!(
            saved
                .iter()
                .find(|record| record.id == survivor)
                .unwrap()
                .goal,
            "after release"
        );
        tip
    };
    let pushed = push::settled_pushes(&mut incoming, &key).await;
    assert_published(
        &handler,
        &pushed,
        &survivor,
        "after release",
        "after release news",
        tip.1,
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .board
            .attention()
            .conversation_watermark(&tip.0)
            .previous,
        Some(tip.1),
        "the successful finisher must consume its agent news"
    );
}

#[tokio::test]
async fn git_action_orchestrator_error_keeps_owner_and_later_publication_live() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut incoming, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (failed, survivor) = {
        let mut app = state.lock().unwrap();
        let failed = adopted_run(&mut app, &repo, dir.path(), "refused-git-owner");
        let survivor = adopted_run(&mut app, &repo, dir.path(), "after-refused-git");
        (failed, survivor)
    };
    push::settled_pushes(&mut incoming, &key).await;
    let (retained, tip) = {
        let mut app = state.lock().unwrap();
        // A project can become unavailable to Git after its run was loaded.
        // Reach the action's inner refusal, without running any Git command.
        app.project_at_mut(0).is_git = false;
        let error = app
            .run_git_action(&json!({"run_id": failed, "action": "commit"}))
            .unwrap_err();
        assert!(error.contains("not a git repository"), "{error}");
        let retained = app.runs.contains_key(&failed);
        app.project_at_mut(0).is_git = true;
        let tip = save_message(
            &mut app,
            &survivor,
            "after refused action",
            "after refused action news",
        );
        (retained, tip)
    };
    let pushed = push::settled_pushes(&mut incoming, &key).await;
    assert_published(
        &handler,
        &pushed,
        &survivor,
        "after refused action",
        "after refused action news",
        tip.1,
    );
    assert!(
        retained,
        "an orchestration refusal must return the borrowed owner"
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .board
            .attention()
            .conversation_watermark(&tip.0)
            .previous,
        Some(tip.1)
    );
    let owner = routed(&handler, "agent.list", json!({"entity_id": failed}));
    assert_eq!(owner["agents"].as_array().unwrap().len(), 1);
}
