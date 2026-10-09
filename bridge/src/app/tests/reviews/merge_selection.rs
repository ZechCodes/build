use super::wire_actions::{merge_params, open};
use super::*;
use crate::reviews::actions::ActionSource;
use crate::reviews::merge::MergeJob;
use crate::reviews::model::ReviewMergeIntent;

#[test]
fn rpc_publication_retry_selects_its_exact_saved_vector_before_a_newer_matching_plan() {
    let home = tempfile::tempdir().unwrap();
    let (_repo_home, mut state, project) = tracked(home.path());
    let extra = init_repo_named(home.path(), "second-source");
    review_call(
        &mut state,
        "project.add_source",
        json!({"project_id":project,"path":extra,"name":"second"}),
    );
    let workspace_id = workspace(&mut state, &project, "saved-vector-selection");
    let directories = state
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .clone();
    for directory in &directories {
        std::fs::write(directory.path.join("reviewed.txt"), "reviewed work\n").unwrap();
        git_in(&directory.path, &["add", "reviewed.txt"]);
        git_in(&directory.path, &["commit", "-m", "saved vector fixture"]);
    }
    let remote = home.path().join("publication.git");
    git_in(
        &directories[0].source_path,
        &["remote", "add", "publish", remote.to_str().unwrap()],
    );
    std::fs::write(
        directories[1].source_path.join("README.md"),
        "dirty target\n",
    )
    .unwrap();
    let pr = open(&mut state, &workspace_id);
    let mut params = merge_params(&pr);
    params["sources"][0]["push"] = json!({"remote":"publish","branch":"main"});
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
    git2::Repository::init_bare(&remote).unwrap();
    git_in(&directories[1].source_path, &["restore", "README.md"]);
    // Older adapters could admit another plan with updated target preconditions.
    // Keep both genuine service results so this assertion exercises RPC selection.
    admit_legacy_newer_plan(&state, &original);
    let before = review_call(
        &mut state,
        "tasks.review.get",
        json!({"task_id":pr["task"]["id"]}),
    );
    assert_eq!(before["merge_intents"].as_array().unwrap().len(), 2);
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .workspace_review_publication_pending(&workspace_id)
        .unwrap());
    params["expected_version"] = before["review"]["version"].clone();
    let retried = review_call(&mut state, "tasks.review.merge", params);
    assert!(
        !state
            .store
            .as_ref()
            .unwrap()
            .workspace_review_publication_pending(&workspace_id)
            .unwrap(),
        "retry must settle the selected original plan, even when a newer plan succeeded"
    );
    let selected = retried["merge_intents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|intent| intent["request_id"] == original["request_id"])
        .unwrap();
    assert_eq!(
        selected["action_ids"].as_array().unwrap().len(),
        original["action_ids"].as_array().unwrap().len() + 1,
        "the publication result must be linked to the original saved plan"
    );
    assert_eq!(selected["request"], original["request"]);
    assert_eq!(
        merge_steps(&retried["review"]),
        merge_steps(&before["review"])
    );
}

fn admit_legacy_newer_plan(state: &AppState, original: &Value) {
    let saved: ReviewMergeIntent = serde_json::from_value(original.clone()).unwrap();
    let store = state.store.as_ref().unwrap();
    let review = store.load_review(&saved.request.task_id).unwrap().unwrap();
    let mut request = saved.request;
    request.expected_version = review.version;
    request.actor = crate::tracker::Actor::User;
    for source in &mut request.sources {
        let binding = review
            .bindings
            .iter()
            .find(|row| row.directory_id == source.directory_id)
            .unwrap();
        source.expected_base_head = git2::Repository::open(&binding.source_repository)
            .unwrap()
            .refname_to_id(&source.base_branch_ref)
            .unwrap()
            .to_string();
    }
    let job = MergeJob {
        project_path: saved.project_path,
        request_id: "legacy-newer-plan".into(),
        request,
        sources: review
            .snapshots
            .last()
            .unwrap()
            .directories
            .iter()
            .map(|directory| ActionSource {
                directory: directory.clone(),
                source_path: directory.source_path.clone(),
                error: None,
            })
            .collect(),
    };
    let completed = crate::reviews::merge::merge(store, &job, || {}).unwrap();
    assert_eq!(
        completed.pull_request.unwrap().status,
        crate::reviews::model::PullRequestStatus::Merged
    );
}

fn merge_steps(review: &Value) -> usize {
    review["actions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["steps"].as_array().unwrap())
        .filter(|step| step["kind"] == "merge")
        .count()
}
