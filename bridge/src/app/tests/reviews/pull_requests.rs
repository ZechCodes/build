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
