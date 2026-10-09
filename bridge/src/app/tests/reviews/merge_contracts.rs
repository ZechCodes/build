use super::wire_actions::{merge_params, open};
use super::*;

fn count_merges(review: &Value) -> usize {
    review["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|action| action["steps"].as_array().unwrap())
        .filter(|step| step["kind"] == "merge" && step["status"] == "succeeded")
        .count()
}

#[test]
fn rpc_merge_retry_retains_partial_plan_and_only_retries_external_publication() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "partial-merge");
    review_call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id":workspace_id,"locked":true}),
    );
    let directory = state.workspaces.get(&workspace_id).unwrap().directories[0].clone();
    let remote = home.path().join("publication.git");
    git_in(
        &directory.source_path,
        &["remote", "add", "publish", remote.to_str().unwrap()],
    );
    std::fs::write(directory.path.join("published.txt"), "reviewed content\n").unwrap();
    git_in(&directory.path, &["add", "published.txt"]);
    git_in(&directory.path, &["commit", "-m", "partial merge fixture"]);
    let pr = open(&mut state, &workspace_id);
    let mut params = merge_params(&pr);
    params["sources"][0]["push"] = json!({"remote":"publish","branch":"main"});
    let failed = review_call(&mut state, "tasks.review.merge", params.clone());
    assert_eq!(
        failed["review"]["pull_request"]["status"], "merged",
        "{failed}"
    );
    assert_eq!(failed["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(count_merges(&failed["review"]), 1);
    assert!(failed["review"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|action| action["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["kind"] == "push" && step["status"] == "failed")));
    git2::Repository::init_bare(&remote).unwrap();
    params["expected_version"] = failed["review"]["version"].clone();
    let retried = review_call(&mut state, "tasks.review.merge", params);
    assert_eq!(retried["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(
        retried["merge_intents"][0]["request_id"],
        failed["merge_intents"][0]["request_id"]
    );
    assert_eq!(
        retried["merge_intents"][0]["request"],
        failed["merge_intents"][0]["request"]
    );
    assert_eq!(count_merges(&retried["review"]), 1);
    let remote_head = git2::Repository::open_bare(remote)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    let base_head = git2::Repository::open(directory.source_path)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .unwrap();
    assert_eq!(remote_head, base_head);
}

#[test]
fn merge_plan_identity_canonicalizes_source_order_and_retains_actor_handoffs() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let extra = init_repo_named(home.path(), "second-source");
    review_call(
        &mut state,
        "project.add_source",
        json!({"project_id":project,"path":extra,"name":"second"}),
    );
    let workspace_id = workspace(&mut state, &project, "identity-merge");
    let directories = state
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .clone();
    for directory in &directories {
        std::fs::write(directory.path.join("reviewed.txt"), "reviewed content\n").unwrap();
        git_in(&directory.path, &["add", "reviewed.txt"]);
        git_in(&directory.path, &["commit", "-m", "identity fixture"]);
    }
    let pr = open(&mut state, &workspace_id);
    assert_eq!(pr["review"]["bindings"].as_array().unwrap().len(), 2);
    let mut params = merge_params(&pr);
    for directory in &directories {
        std::fs::write(directory.source_path.join("README.md"), "dirty source\n").unwrap();
    }
    let first = review_call(&mut state, "tasks.review.merge", params.clone());
    assert_eq!(first["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(first["merge_intents"][0]["state"], "failed", "{first}");
    params["expected_version"] = first["review"]["version"].clone();
    params["sources"].as_array_mut().unwrap().reverse();
    let reordered = review_call(&mut state, "tasks.review.merge", params.clone());
    assert_eq!(reordered["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(
        reordered["merge_intents"][0]["request_id"],
        first["merge_intents"][0]["request_id"]
    );
    let (owner, agent) = project_agent(&mut state, &project);
    params["expected_version"] = reordered["review"]["version"].clone();
    let action = crate::mcp::BridgeAction::TrackerMergeReview {
        params: serde_json::from_value(params).unwrap(),
    };
    let other = state.agent_action(&owner, &agent, action).unwrap();
    let plans = other["merge_intents"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "{other}");
    assert_eq!(
        plans[0]["request_id"],
        first["merge_intents"][0]["request_id"]
    );
    assert_eq!(plans[0]["request"]["actor"]["kind"], "user");
}
