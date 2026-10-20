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

fn retain_legacy_release_path(app: &mut AppState, owner: &str) {
    // Persisted adopted owners can predate workspace maintenance. Recreate
    // that supported shape even when normal adoption eagerly adds its alias.
    let root = app.runs[owner].worktree.path.clone();
    let aliases: Vec<_> = app
        .workspaces
        .list(None)
        .into_iter()
        .filter(|workspace| workspace.id == owner || workspace.root == root)
        .map(|workspace| {
            assert!(
                !workspace.managed,
                "the fixture only removes compatibility aliases"
            );
            workspace.id.clone()
        })
        .collect();
    for alias in aliases {
        app.workspaces.forget(&alias);
    }
    assert!(app.surviving_workspace_of_run(owner).is_none());
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
        retain_legacy_release_path(&mut app, &released);
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

#[tokio::test]
async fn adopted_implementation_qa_error_keeps_owner_and_later_publication_live() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut incoming, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (failed, survivor, plan) = {
        let mut app = state.lock().unwrap();
        let failed = adopted_run(&mut app, &repo, dir.path(), "qa-failed-owner");
        let survivor = adopted_run(&mut app, &repo, dir.path(), "after-qa-refusal");
        let plan = file_legacy_task(&mut app, "implementation with refused QA output");
        let path = app.runs[&failed].worktree.path.join("result.txt");
        std::fs::create_dir(path).unwrap();
        (failed, survivor, plan)
    };
    push::settled_pushes(&mut incoming, &key).await;
    let (retained, tip) = {
        let mut app = state.lock().unwrap();
        let project = app.project_of(&failed).unwrap();
        let choice = app.runs[&failed].model_choice.clone();
        let base_sha = crate::worktree::git_stdout(&repo, &["rev-parse", "HEAD"]).unwrap();
        let prepared = crate::lifecycle::AdoptedImplementation {
            base_sha,
            adopted: None,
        };
        let error = app
            .open_adopted_implementation(project, plan, failed.clone(), prepared, choice)
            .unwrap_err();
        assert!(
            error.contains("directory"),
            "the QA output path must refuse its write: {error}"
        );
        let retained = app.runs.contains_key(&failed);
        let tip = save_message(
            &mut app,
            &survivor,
            "after QA refusal",
            "after QA refusal news",
        );
        (retained, tip)
    };
    let pushed = push::settled_pushes(&mut incoming, &key).await;
    assert_published(
        &handler,
        &pushed,
        &survivor,
        "after QA refusal",
        "after QA refusal news",
        tip.1,
    );
    assert!(
        retained,
        "a QA refusal must return the existing adopted owner"
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
}

#[tokio::test]
async fn joined_dispatch_settlement_error_keeps_owner_and_later_publication_live() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut incoming, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (failed, survivor, project) = {
        let mut app = state.lock().unwrap();
        let failed = adopted_run(&mut app, &repo, dir.path(), "dispatch-failed-owner");
        let survivor = adopted_run(&mut app, &repo, dir.path(), "after-dispatch-refusal");
        let project = app.project_of(&failed).unwrap();
        (failed, survivor, project)
    };
    push::settled_pushes(&mut incoming, &key).await;
    state.lock().unwrap().dispatch_fault = Some(BranchDispatchStep::Settle);
    let refused = handler.call(sender, req("branch.dispatch", json!({
        "project_id": project, "branch": "dispatch-failed-owner", "instruction": "refused instruction"
    })));
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("Settle"),
        "{refused}"
    );
    let (retained, tip) = {
        let mut app = state.lock().unwrap();
        app.dispatch_fault = None;
        let retained = app.runs.contains_key(&failed);
        assert!(
            app.delivery_queue.queued_is_empty(),
            "a refused instruction must not be delivered"
        );
        let tip = save_message(
            &mut app,
            &survivor,
            "after dispatch refusal",
            "after dispatch refusal news",
        );
        (retained, tip)
    };
    let pushed = push::settled_pushes(&mut incoming, &key).await;
    assert_published(
        &handler,
        &pushed,
        &survivor,
        "after dispatch refusal",
        "after dispatch refusal news",
        tip.1,
    );
    assert!(
        retained,
        "an injected settlement refusal must return the joined owner"
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
}

#[test]
fn merge_release_keeps_unrelated_routed_reads_live() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let released = adopted_run(&mut app, &repo, dir.path(), "release-read-owner");
    let survivor = adopted_run(&mut app, &repo, dir.path(), "release-read-survivor");
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    save_message(
        &mut state.lock().unwrap(),
        &survivor,
        "read before release",
        "before",
    );
    let before = routed(&handler, "board.list", json!({}));
    assert!(
        serde_json::to_string(&before)
            .unwrap()
            .contains("read before release"),
        "{before}"
    );
    {
        let mut app = state.lock().unwrap();
        let project = app.project_of(&released).unwrap();
        retain_legacy_release_path(&mut app, &released);
        let worktree = app.runs[&released].worktree.clone();
        app.apply_merge_cleanup(&released, &project, &worktree, MergeCleanup::Release);
        assert!(!app.runs.contains_key(&released));
        save_message(&mut app, &survivor, "read after release", "after");
    }
    let after = routed(&handler, "board.list", json!({}));
    assert!(
        serde_json::to_string(&after)
            .unwrap()
            .contains("read after release"),
        "successful unrelated write did not reach routed read: {after}"
    );
}

fn fresh_implementation_adoption(
    app: &mut AppState,
    repo: &std::path::Path,
    dir: &std::path::Path,
    owner: &str,
) -> crate::lifecycle::AdoptedImplementation {
    add_external_worktree(repo, dir, owner, owner);
    let project_id = app.project_at(0).id.clone();
    let base_branch = app.base_for(&project_id).unwrap();
    let checkout = app
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.branch.as_deref() == Some(owner))
        .unwrap();
    let base_sha = crate::worktree::git_stdout(&checkout.path, &["rev-parse", "HEAD"]).unwrap();
    crate::lifecycle::AdoptedImplementation {
        base_sha,
        adopted: Some(crate::lifecycle::AdoptionPrepared {
            project_id,
            run_id: owner.to_string(),
            base_branch: base_branch.clone(),
            checkout: crate::orchestrator::AdoptableCheckout::judge(&checkout, &base_branch)
                .unwrap(),
            model_choice: ModelChoice::default(),
        }),
    }
}

fn assert_adopted_workspace_published(app: &AppState, owner: &str) {
    assert!(app.runs.contains_key(owner), "the adopted owner was saved");
    let detail = app
        .snapshot_service()
        .call(req("workspace.get", json!({"workspace_id": owner})));
    assert_eq!(
        detail["ok"], true,
        "the saved adoption must publish its alias: {detail}"
    );
    assert_eq!(detail["result"]["run_id"], owner, "{detail}");
}

#[test]
fn adopted_implementation_publishes_its_workspace_before_acknowledgement() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project = app.project_at(0).id.clone();
    let task = file_legacy_task(&mut app, "implementation on a newly adopted checkout");
    let owner = "new-implementation-owner";
    let adoption = fresh_implementation_adoption(&mut app, &repo, dir.path(), owner);

    app.open_adopted_implementation(
        project,
        task,
        owner.to_string(),
        adoption,
        ModelChoice::default(),
    )
    .unwrap();

    assert_adopted_workspace_published(&app, owner);
}

#[test]
fn refused_implementation_handover_publishes_the_adoption_it_retains() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project = app.project_at(0).id.clone();
    let owner = "refused-implementation-owner";
    let adoption = fresh_implementation_adoption(&mut app, &repo, dir.path(), owner);

    let error = app
        .open_adopted_implementation(
            project,
            "missing-plan".to_string(),
            owner.to_string(),
            adoption,
            ModelChoice::default(),
        )
        .unwrap_err();
    assert_eq!(error, "unknown plan_id");

    assert_adopted_workspace_published(&app, owner);
}
