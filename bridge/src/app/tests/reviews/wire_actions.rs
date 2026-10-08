use super::*;

pub(super) fn open(state: &mut AppState, workspace_id: &str) -> Value {
    review_call(
        state,
        "tasks.review.open",
        json!({
            "workspace_id":workspace_id,"request_id":"wire-actions","title":"Review committed work",
            "description":"Verify the public action adapters"
        }),
    )
}

pub(super) fn merge_params(pr: &Value) -> Value {
    let sources = pr["review"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|binding| {
            let repo =
                git2::Repository::open(binding["source_repository"].as_str().unwrap()).unwrap();
            let head = repo
                .refname_to_id(binding["base_branch_ref"].as_str().unwrap())
                .unwrap();
            json!({"directory_id":binding["directory_id"],"expected_base_head":head.to_string()})
        })
        .collect::<Vec<_>>();
    json!({"task_id":pr["task"]["id"],"expected_version":pr["review"]["version"],
        "snapshot_id":pr["review"]["snapshots"].as_array().unwrap().last().unwrap()["id"],"sources":sources})
}

#[test]
fn rpc_push_and_base_update_publish_user_attributed_pinned_snapshots() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "public-push-update");
    let pr = open(&mut state, &workspace_id);
    let binding = &pr["review"]["bindings"][0];
    let working = std::path::Path::new(binding["working_repository"].as_str().unwrap());
    let source = std::path::Path::new(binding["source_repository"].as_str().unwrap());
    git_in(source, &["branch", "release"]);
    std::fs::write(working.join("api-change.txt"), "committed change\n").unwrap();
    git_in(working, &["add", "api-change.txt"]);
    git_in(working, &["commit", "-m", "API push fixture"]);
    let head = git2::Repository::open(working)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    let pushed = review_call(
        &mut state,
        "tasks.review.push",
        json!({
            "task_id":pr["task"]["id"],"expected_version":1,
            "sources":[{"directory_id":binding["directory_id"],"expected_head":head,
                "expected_received_head":binding["last_received_head"]}]
        }),
    );
    assert_eq!(pushed["sources"][0]["status"], "published", "{pushed}");
    assert_eq!(pushed["review"]["version"], 2);
    assert_eq!(
        pushed["review"]["snapshots"][1]["directories"][0]["head"],
        head
    );
    assert_eq!(pushed["review"]["snapshots"][1]["author"]["kind"], "user");
    let updated = review_call(
        &mut state,
        "tasks.review.update",
        json!({
            "task_id":pr["task"]["id"],"expected_version":2,
            "bases":[{"directory_id":binding["directory_id"],"branch":"release"}]
        }),
    );
    assert_eq!(updated["review"]["version"], 3);
    assert_eq!(
        updated["review"]["bindings"][0]["base_branch_ref"],
        "refs/heads/release"
    );
    assert_eq!(updated["review"]["snapshots"][2]["author"]["kind"], "user");
    assert_eq!(
        updated["review"]["snapshots"][2]["directories"][0]["head"],
        head
    );
    assert_eq!(
        updated["review"]["snapshots"][2]["publication"]["reason"],
        "base_changed"
    );
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review(pr["task"]["id"].as_str().unwrap())
            .unwrap()
            .unwrap()
            .snapshots
            .len(),
        3
    );
}

#[test]
fn rpc_pr_snapshot_refuses_workspace_replacement_without_changing_history() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "original-review");
    let replacement = workspace(&mut state, &project, "replacement-review");
    let pr = open(&mut state, &workspace_id);
    let task_id = pr["task"]["id"].as_str().unwrap();
    let before = state.store.as_ref().unwrap().load_review(task_id).unwrap();
    let refused = state.handle(req(
        "tasks.review.snapshot",
        json!({
            "task_id":task_id,"workspace_id":replacement,"expected_version":1
        }),
    ));
    assert_eq!(refused["error_code"], "invalid_params", "{refused}");
    assert_eq!(refused["details"]["workspace_id"], workspace_id);
    assert!(refused["details"]["recovery"]
        .as_str()
        .unwrap()
        .contains("open a new PR"));
    assert_eq!(
        state.store.as_ref().unwrap().load_review(task_id).unwrap(),
        before
    );
}
