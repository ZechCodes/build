use super::*;

fn opened(state: &mut AppState, workspace_id: &str) -> Value {
    review_call(
        state,
        "tasks.review.open",
        json!({
            "workspace_id": workspace_id, "request_id": "rpc-pr-open", "title":"Review API",
            "description":"Review the committed changes",
        }),
    )
}

fn published_review_status(state: &AppState, workspace: &str) -> Value {
    state
        .read_view
        .load()
        .get("workspace.get", workspace)
        .unwrap()["active_review"]["status"]
        .clone()
}

fn published_task_review_status(state: &AppState, task: &str) -> Value {
    state.read_view.load().get("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["task_id"] == task)
        .unwrap()["review_summary"]["status"]
        .clone()
}

#[test]
fn pr_close_publishes_committed_workspace_review_summary() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "close-publication");
    let pr = opened(&mut state, &workspace_id);
    state.initialize_snapshot_projection();
    assert_eq!(published_review_status(&state, &workspace_id), "open");
    review_call(
        &mut state,
        "tasks.review.close",
        json!({
            "task_id": pr["task"]["id"], "expected_version": 1, "description": "Deferred"
        }),
    );
    assert_eq!(published_review_status(&state, &workspace_id), "closed");
}

#[test]
fn pr_reopen_publishes_committed_workspace_review_summary() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "reopen-publication");
    let pr = opened(&mut state, &workspace_id);
    review_call(
        &mut state,
        "tasks.review.close",
        json!({
            "task_id": pr["task"]["id"], "expected_version": 1, "description": "Deferred"
        }),
    );
    state.initialize_snapshot_projection();
    assert_eq!(published_review_status(&state, &workspace_id), "closed");
    review_call(
        &mut state,
        "tasks.review.reopen",
        json!({
            "task_id": pr["task"]["id"], "expected_version": 2
        }),
    );
    assert_eq!(published_review_status(&state, &workspace_id), "open");
}

#[test]
fn pr_merge_publishes_committed_workspace_review_summary() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "merge-publication");
    let pr = opened(&mut state, &workspace_id);
    state.initialize_snapshot_projection();
    assert_eq!(published_review_status(&state, &workspace_id), "open");
    let params = super::wire_actions::merge_params(&pr);
    review_call(&mut state, "tasks.review.merge", params);
    assert_eq!(published_review_status(&state, &workspace_id), "merged");
}

#[tokio::test]
async fn background_review_sync_publishes_committed_workspace_review_summary() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "background-publication");
    let pr = opened(&mut state, &workspace_id);
    let working = std::path::Path::new(
        pr["review"]["bindings"][0]["working_repository"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(working.join("background.txt"), "new review content\n").unwrap();
    git_in(working, &["add", "background.txt"]);
    git_in(working, &["commit", "-m", "background publication"]);
    git_in(working, &["push"]);
    let task = pr["task"]["id"].as_str().unwrap().to_owned();
    let store = state.store.as_ref().unwrap().clone();
    let shared = state.shared();
    let view = shared.lock().unwrap().read_view.clone();
    let sync = AppState::spawn_review_sync(Arc::clone(&shared))
        .await
        .unwrap();
    let publication = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if view.load().get("workspace.get", &workspace_id).unwrap()["active_review"]["version"]
                .as_u64()
                .unwrap()
                > 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    drop(sync);
    assert!(
        store.load_review(&task).unwrap().unwrap().version > 1,
        "background reconciliation must commit before checking publication"
    );
    assert!(
        publication.is_ok(),
        "background commit left the published workspace review stale"
    );
}

#[test]
fn merge_progress_publishes_committed_workspace_review_summary_before_apply() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "merge-progress-publication");
    let pr = opened(&mut state, &workspace_id);
    let task = pr["task"]["id"].as_str().unwrap();
    review_call(&mut state, "tasks.watch", json!({"task_id": task}));
    let params = super::wire_actions::merge_params(&pr);
    let shared = state.shared();
    let work = {
        let mut app = shared.lock().unwrap();
        app.review_merge(
            serde_json::from_value(params.clone()).unwrap(),
            crate::tracker::Actor::User,
        )
        .unwrap();
        app.take_deferred().unwrap()
    };
    let done = work.run();
    let mut app = shared.lock().unwrap();
    let published = published_review_status(&app, &workspace_id);
    let published_task = published_task_review_status(&app, task);
    let saved = app
        .store
        .as_ref()
        .unwrap()
        .load_review(pr["task"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    app.apply_deferred("tasks.review.merge", &params, done)
        .unwrap();
    assert_eq!(
        saved.pull_request.unwrap().status,
        crate::reviews::model::PullRequestStatus::Merged
    );
    assert_eq!(
        published, "merged",
        "merge progress must publish before the deferred final apply"
    );
    assert_eq!(
        published_task, "merged",
        "merge progress must publish the watched task before final apply"
    );
}

#[test]
fn pr_refresh_snapshot_close_and_reopen_use_the_bound_workspace_and_preserve_history() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "pr-lifecycle");
    let pr = opened(&mut state, &workspace_id);
    let task_id = pr["task"]["id"].as_str().unwrap();
    let refreshed = review_call(
        &mut state,
        "tasks.review.refresh",
        json!({"task_id":task_id,"expected_version":1}),
    );
    assert_eq!(refreshed["review"]["version"], 1);
    let saved = review_call(
        &mut state,
        "tasks.review.snapshot",
        json!({"task_id":task_id,"workspace_id":workspace_id,"expected_version":1}),
    );
    assert_eq!(saved["review"]["version"], 1);
    let invalid = state.handle(req("tasks.review.snapshot", json!({"task_id":task_id,"workspace_id":workspace_id,"expected_version":1,"base_overrides":{"dir":"main"}})));
    assert_eq!(invalid["error_code"], "invalid_params", "{invalid}");
    let stale = state.handle(req(
        "tasks.review.close",
        json!({"task_id":task_id,"expected_version":0,"description":"Closed"}),
    ));
    assert_eq!(stale["error_code"], "stale_version", "{stale}");
    let closed = review_call(
        &mut state,
        "tasks.review.close",
        json!({"task_id":task_id,"expected_version":1,"description":"Deferred"}),
    );
    assert_eq!(closed["review"]["pull_request"]["status"], "closed");
    let reopened = review_call(
        &mut state,
        "tasks.review.reopen",
        json!({"task_id":task_id,"expected_version":2}),
    );
    assert_eq!(reopened["review"]["pull_request"]["status"], "open");
    assert_eq!(reopened["review"]["snapshots"].as_array().unwrap().len(), 2);
    assert!(state.workspaces.get(&workspace_id).is_some());
}

#[test]
fn pr_merge_derives_bound_destination_and_keeps_a_locked_workspace() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "pr-merge");
    review_call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id":workspace_id,"locked":true}),
    );
    let pr = opened(&mut state, &workspace_id);
    let task_id = pr["task"]["id"].as_str().unwrap();
    let binding = &pr["review"]["bindings"][0];
    let source = std::path::Path::new(binding["source_repository"].as_str().unwrap());
    let base = git2::Repository::open(source)
        .unwrap()
        .refname_to_id(binding["base_branch_ref"].as_str().unwrap())
        .unwrap();
    let merged = review_call(
        &mut state,
        "tasks.review.merge",
        json!({
            "task_id":task_id,"expected_version":1,"snapshot_id":pr["review"]["snapshots"][0]["id"],
            "sources":[{"directory_id":binding["directory_id"],"expected_base_head":base.to_string()}]
        }),
    );
    assert_eq!(
        merged["review"]["pull_request"]["status"], "merged",
        "{merged}"
    );
    assert!(state.workspaces.get(&workspace_id).unwrap().locked);
    let refused = state.handle(req(
        "tasks.review.reopen",
        json!({"task_id":task_id,"expected_version":merged["review"]["version"]}),
    ));
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert!(refused["details"]["recovery"].is_string());
}

#[test]
fn pr_refresh_reports_unavailable_received_refs_with_independent_observations() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "pr-refresh-failure");
    let pr = opened(&mut state, &workspace_id);
    let binding = &pr["review"]["bindings"][0];
    let receiver =
        git2::Repository::open_bare(binding["receiving_repository"].as_str().unwrap()).unwrap();
    receiver
        .find_reference(binding["receiving_ref"].as_str().unwrap())
        .unwrap()
        .delete()
        .unwrap();
    let refreshed = review_call(
        &mut state,
        "tasks.review.refresh",
        json!({"task_id":pr["task"]["id"],"expected_version":1}),
    );
    assert_eq!(refreshed["review"]["version"], 1);
    assert_eq!(refreshed["sync"][0]["health"], "unavailable");
    assert!(refreshed["sync"][0]["revision"].as_u64().unwrap() > 0);
    assert!(refreshed["sync"][0]["error"].is_string());
}

#[test]
fn pr_merge_can_refresh_a_failed_unintegrated_plan_after_a_same_snapshot_opinion() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "pr-merge-replan");
    review_call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id":workspace_id,"locked":true}),
    );
    let working = state.workspaces.get(&workspace_id).unwrap().directories[0]
        .path
        .clone();
    std::fs::write(working.join("change.txt"), "reviewed change\n").unwrap();
    git_in(&working, &["add", "change.txt"]);
    git_in(&working, &["commit", "-m", "reviewed change"]);
    let pr = opened(&mut state, &workspace_id);
    let task_id = pr["task"]["id"].as_str().unwrap();
    let binding = &pr["review"]["bindings"][0];
    let source = std::path::Path::new(binding["source_repository"].as_str().unwrap());
    let base = git2::Repository::open(source)
        .unwrap()
        .refname_to_id(binding["base_branch_ref"].as_str().unwrap())
        .unwrap();
    let mut params = json!({"task_id":task_id,"expected_version":1,"snapshot_id":pr["review"]["snapshots"][0]["id"],
        "sources":[{"directory_id":binding["directory_id"],"expected_base_head":base.to_string()}]});
    std::fs::write(source.join("README.md"), "uncommitted source change\n").unwrap();
    let failed = review_call(&mut state, "tasks.review.merge", params.clone());
    assert_eq!(failed["merge_intents"][0]["state"], "failed", "{failed}");
    review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":task_id,"body":"Ready after resolving the source change",
        "opinion":{"snapshot_id":params["snapshot_id"],"verdict":"approve"}}),
    );
    git_in(source, &["checkout", "--", "README.md"]);
    let current = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    params["expected_version"] = current["review"]["version"].clone();
    let merged = review_call(&mut state, "tasks.review.merge", params.clone());
    assert_eq!(
        merged["review"]["pull_request"]["status"], "merged",
        "{merged}"
    );
    assert_eq!(merged["merge_intents"].as_array().unwrap().len(), 2);
    params["expected_version"] = merged["review"]["version"].clone();
    let retried = review_call(&mut state, "tasks.review.merge", params);
    assert_eq!(retried["merge_intents"], merged["merge_intents"]);
    assert_eq!(retried["review"]["actions"], merged["review"]["actions"]);
}
