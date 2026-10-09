use super::wire_actions::{merge_params, open};
use super::*;

fn successful_merges(review: &Value, directory_id: &Value) -> usize {
    review["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["directory_id"] == *directory_id)
        .flat_map(|row| row["steps"].as_array().unwrap())
        .filter(|step| step["kind"] == "merge" && step["status"] == "succeeded")
        .count()
}

#[test]
fn browser_retries_an_agent_owned_legacy_merge_publication_without_reintegrating() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "agent-merge-handoff");
    let directory = state.workspaces.get(&workspace_id).unwrap().directories[0].clone();
    let remote = home.path().join("publication.git");
    git_in(
        &directory.source_path,
        &["remote", "add", "publish", remote.to_str().unwrap()],
    );
    std::fs::write(directory.path.join("handoff.txt"), "reviewed work\n").unwrap();
    git_in(&directory.path, &["add", "handoff.txt"]);
    git_in(&directory.path, &["commit", "-m", "agent merge handoff"]);
    let pr = open(&mut state, &workspace_id);
    let mut params = merge_params(&pr);
    params["sources"][0]["push"] = json!({"remote":"publish","branch":"main"});
    let (owner, agent) = project_agent(&mut state, &project);
    let merged = state
        .agent_action(
            &owner,
            &agent,
            crate::mcp::BridgeAction::TrackerMergeReview {
                params: serde_json::from_value(params.clone()).unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        merged["review"]["pull_request"]["status"], "merged",
        "{merged}"
    );
    assert_eq!(
        merged["merge_intents"][0]["request"]["actor"]["agent_id"],
        agent
    );
    let original = merged["merge_intents"][0].clone();
    git2::Repository::init_bare(&remote).unwrap();
    params["expected_version"] = merged["review"]["version"].clone();
    let published = review_call(&mut state, "tasks.review.merge", params);
    assert_eq!(published["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(
        published["merge_intents"][0]["request_id"],
        original["request_id"]
    );
    assert_eq!(
        published["merge_intents"][0]["request"],
        original["request"]
    );
    assert_eq!(
        successful_merges(
            &published["review"],
            &pr["review"]["bindings"][0]["directory_id"]
        ),
        1
    );
    assert!(published["review"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["steps"].as_array().unwrap())
        .any(|step| step["kind"] == "push" && step["status"] == "succeeded"));
}

#[test]
fn rpc_partial_merge_replans_after_opinion_and_target_changes_without_losing_saved_work() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let extra = init_repo_named(home.path(), "second-source");
    review_call(
        &mut state,
        "project.add_source",
        json!({"project_id":project,"path":extra,"name":"second"}),
    );
    let workspace_id = workspace(&mut state, &project, "partial-recovery");
    review_call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id":workspace_id,"locked":true}),
    );
    let directories = state
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .clone();
    let remotes = directories
        .iter()
        .enumerate()
        .map(|(index, directory)| {
            let remote = home.path().join(format!("publication-{index}.git"));
            git_in(
                &directory.source_path,
                &["remote", "add", "publish", remote.to_str().unwrap()],
            );
            std::fs::write(directory.path.join("reviewed.txt"), "reviewed work\n").unwrap();
            git_in(&directory.path, &["add", "reviewed.txt"]);
            git_in(&directory.path, &["commit", "-m", "partial recovery"]);
            remote
        })
        .collect::<Vec<_>>();
    let pr = open(&mut state, &workspace_id);
    let mut params = merge_params(&pr);
    for source in params["sources"].as_array_mut().unwrap() {
        source["push"] = json!({"remote":"publish","branch":"main"});
    }
    let second = &directories[1];
    std::fs::write(second.source_path.join("README.md"), "dirty source\n").unwrap();
    let partial = review_call(&mut state, "tasks.review.merge", params.clone());
    assert!(
        partial["review"]["pull_request"]["status"] != "merged",
        "{partial}"
    );
    let original = partial["merge_intents"][0].clone();
    assert_eq!(
        successful_merges(
            &partial["review"],
            &pr["review"]["bindings"][0]["directory_id"]
        ),
        1
    );
    review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":pr["task"]["id"],"body":"Approve repaired sources",
        "opinion":{"snapshot_id":params["snapshot_id"],"verdict":"approve"}}),
    );
    git_in(&second.source_path, &["restore", "README.md"]);
    std::fs::write(second.source_path.join("base-next.txt"), "new base work\n").unwrap();
    git_in(&second.source_path, &["add", "base-next.txt"]);
    git_in(&second.source_path, &["commit", "-m", "base advanced"]);
    for remote in &remotes {
        git2::Repository::init_bare(remote).unwrap();
    }
    let current = review_call(
        &mut state,
        "tasks.review.get",
        json!({"task_id":pr["task"]["id"]}),
    );
    let mut refreshed = merge_params(&json!({"review":current["review"],"task":pr["task"]}));
    for source in refreshed["sources"].as_array_mut().unwrap() {
        source["push"] = json!({"remote":"publish","branch":"main"});
    }
    let mut altered_publication = refreshed.clone();
    altered_publication["sources"][0]["push"]["branch"] = json!("different-destination");
    let refused = state.handle(req("tasks.review.merge", altered_publication));
    assert_eq!(
        refused["ok"], false,
        "original publication obligations must remain fixed: {refused}"
    );
    let mut stale = refreshed.clone();
    stale["expected_version"] = params["expected_version"].clone();
    let refused = state.handle(req("tasks.review.merge", stale));
    assert_eq!(refused["ok"], false, "{refused}");
    let recovered = review_call(&mut state, "tasks.review.merge", refreshed.clone());
    assert_recovered_plan(&recovered, &original, &pr, &refreshed);
    assert!(!state
        .store
        .as_ref()
        .unwrap()
        .workspace_review_publication_pending(&workspace_id)
        .unwrap());
}

fn assert_recovered_plan(recovered: &Value, original: &Value, pr: &Value, refreshed: &Value) {
    assert_eq!(
        recovered["review"]["pull_request"]["status"], "merged",
        "{recovered}"
    );
    assert_eq!(recovered["merge_intents"].as_array().unwrap().len(), 1);
    let saved = &recovered["merge_intents"][0];
    assert_eq!(saved["request_id"], original["request_id"]);
    assert_eq!(saved["request"]["actor"], original["request"]["actor"]);
    assert!(saved["action_ids"]
        .as_array()
        .unwrap()
        .starts_with(original["action_ids"].as_array().unwrap()));
    for (index, binding) in pr["review"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(
            successful_merges(&recovered["review"], &binding["directory_id"]),
            1
        );
        assert_eq!(
            saved["request"]["sources"][index]["push"],
            original["request"]["sources"][index]["push"]
        );
    }
    assert_eq!(
        saved["request"]["sources"][0]["expected_base_head"],
        original["request"]["sources"][0]["expected_base_head"]
    );
    assert_eq!(
        saved["request"]["sources"][1]["expected_base_head"],
        refreshed["sources"][1]["expected_base_head"]
    );
}

#[test]
fn browser_publishes_an_agent_partial_merge_after_a_newer_active_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let extra = init_repo_named(home.path(), "historical-second-source");
    review_call(
        &mut state,
        "project.add_source",
        json!({"project_id":project,"path":extra,"name":"second"}),
    );
    let workspace_id = workspace(&mut state, &project, "historical-agent-publication");
    let directories = state
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .clone();
    let remote = home.path().join("historical-publication.git");
    for directory in &directories {
        git_in(
            &directory.source_path,
            &["remote", "add", "publish", remote.to_str().unwrap()],
        );
        std::fs::write(directory.path.join("reviewed.txt"), "reviewed work\n").unwrap();
        git_in(&directory.path, &["add", "reviewed.txt"]);
        git_in(&directory.path, &["commit", "-m", "historical recovery"]);
    }
    let pr = open(&mut state, &workspace_id);
    let mut params = merge_params(&pr);
    params["sources"][0]["push"] = json!({"remote":"publish","branch":"main"});
    std::fs::write(
        directories[1].source_path.join("README.md"),
        "dirty source\n",
    )
    .unwrap();
    let (owner, agent) = project_agent(&mut state, &project);
    let partial = state
        .agent_action(
            &owner,
            &agent,
            crate::mcp::BridgeAction::TrackerMergeReview {
                params: serde_json::from_value(params.clone()).unwrap(),
            },
        )
        .unwrap();
    let original = partial["merge_intents"][0].clone();
    std::fs::write(
        directories[1].path.join("new-look.txt"),
        "new received work\n",
    )
    .unwrap();
    git_in(&directories[1].path, &["add", "new-look.txt"]);
    git_in(
        &directories[1].path,
        &["commit", "-m", "newer published snapshot"],
    );
    let head = git2::Repository::open(&directories[1].path)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    let pushed = review_call(
        &mut state,
        "tasks.review.push",
        json!({"task_id":pr["task"]["id"],
        "expected_version":partial["review"]["version"],
        "sources":[{"directory_id":directories[1].id,"expected_head":head,
            "expected_received_head":pr["review"]["bindings"][1]["last_received_head"]}]}),
    );
    assert_ne!(
        pushed["review"]["pull_request"]["latest_published_snapshot_id"],
        params["snapshot_id"]
    );
    git2::Repository::init_bare(&remote).unwrap();
    params["expected_version"] = pushed["review"]["version"].clone();
    let published = review_call(&mut state, "tasks.review.merge", params);
    assert_eq!(published["merge_intents"].as_array().unwrap().len(), 1);
    assert_eq!(
        published["merge_intents"][0]["request_id"],
        original["request_id"]
    );
    assert_eq!(
        published["merge_intents"][0]["request"],
        original["request"]
    );
    assert_eq!(
        published["review"]["pull_request"]["latest_published_snapshot_id"],
        pushed["review"]["pull_request"]["latest_published_snapshot_id"]
    );
    assert_eq!(
        successful_merges(
            &published["review"],
            &pr["review"]["bindings"][0]["directory_id"]
        ),
        1
    );
    assert_eq!(
        successful_merges(
            &published["review"],
            &pr["review"]["bindings"][1]["directory_id"]
        ),
        0
    );
    assert!(published["review"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["steps"].as_array().unwrap())
        .any(|step| step["kind"] == "push" && step["status"] == "succeeded"));
}
