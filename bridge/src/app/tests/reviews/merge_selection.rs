use super::wire_actions::{merge_params, open};
use super::*;
use crate::reviews::actions::{ActionSource, ActionStatus, StepKind, StepStatus};
use crate::reviews::merge::MergeJob;
use crate::reviews::model::ReviewMergeIntent;

#[test]
fn rpc_publication_retry_selects_its_exact_saved_vector_before_a_newer_matching_plan() {
    publication_retry(false, None);
}

#[test]
fn rpc_identical_vector_retry_selects_pending_publication_before_a_completed_plan() {
    publication_retry(true, None);
}

#[test]
fn rpc_settled_historical_plan_does_not_shadow_another_identical_pending_plan() {
    publication_retry(true, Some(StepStatus::Failed));
}

#[test]
fn rpc_settled_interrupted_push_does_not_shadow_another_identical_pending_plan() {
    publication_retry(true, Some(StepStatus::Interrupted));
}

fn publication_retry(identical_vector: bool, another_pending_plan: Option<StepStatus>) {
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
    for (index, directory) in directories.iter().enumerate() {
        if identical_vector && index == 0 {
            continue;
        }
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
    let mut pending = vec![original.clone()];
    if let Some(status) = another_pending_plan {
        let extra = admit_legacy_plan(&state, &original, "legacy-pending-plan");
        pending.push(retain_publication_status(&state, extra, status));
    }
    git2::Repository::init_bare(&remote).unwrap();
    git_in(&directories[1].source_path, &["restore", "README.md"]);
    // Older adapters could admit another plan with updated target preconditions.
    // Keep both genuine service results so this assertion exercises RPC selection.
    let completed = admit_legacy_plan(&state, &original, "legacy-newer-plan");
    assert_eq!(completed["state"], "succeeded");
    let before = review_call(
        &mut state,
        "tasks.review.get",
        json!({"task_id":pr["task"]["id"]}),
    );
    assert_eq!(
        before["merge_intents"].as_array().unwrap().len(),
        pending.len() + 1
    );
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .workspace_review_publication_pending(&workspace_id)
        .unwrap());
    let mut current = before.clone();
    // Once the newest pending plan is published, its historical Failed state
    // must not hide the older plan's still-unpublished successful result.
    for saved in pending.iter().rev() {
        params["expected_version"] = current["review"]["version"].clone();
        current = review_call(&mut state, "tasks.review.merge", params.clone());
        assert_saved_publication(&current, saved);
        assert_eq!(
            merge_steps(&current["review"]),
            merge_steps(&before["review"])
        );
    }
    assert!(
        !state
            .store
            .as_ref()
            .unwrap()
            .workspace_review_publication_pending(&workspace_id)
            .unwrap(),
        "retry must settle the selected original plan, even when a newer plan succeeded"
    );
}

fn assert_saved_publication(retried: &Value, original: &Value) {
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
    if original["state"] == "interrupted" {
        let history = retried["review"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| {
                original["action_ids"]
                    .as_array()
                    .unwrap()
                    .contains(&action["id"])
                    && action["steps"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|step| step["kind"] == "push")
            })
            .unwrap();
        let push = history["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["kind"] == "push")
            .unwrap();
        assert_eq!(push["status"], "interrupted", "publication retry preserves the original interrupted Push row before the next intent retry");
    }
}

fn retain_publication_status(state: &AppState, saved: Value, status: StepStatus) -> Value {
    if status != StepStatus::Interrupted {
        return saved;
    }
    let mut intent: ReviewMergeIntent = serde_json::from_value(saved).unwrap();
    let store = state.store.as_ref().unwrap();
    let review = store.load_review(&intent.request.task_id).unwrap().unwrap();
    let mut action = review
        .actions
        .into_iter()
        .find(|row| {
            intent.action_ids.contains(&row.id)
                && row.steps.iter().any(|step| step.kind == StepKind::Push)
        })
        .unwrap();
    action.status = ActionStatus::Interrupted;
    let push = action
        .steps
        .iter_mut()
        .find(|step| step.kind == StepKind::Push)
        .unwrap();
    push.status = StepStatus::Interrupted;
    store
        .save_review_action(&intent.request.task_id, &action)
        .unwrap();
    intent.state = crate::reviews::model::ReviewMergeState::Interrupted;
    serde_json::to_value(
        store
            .save_review_merge_intent(&intent, intent.version)
            .unwrap(),
    )
    .unwrap()
}

fn admit_legacy_plan(state: &AppState, original: &Value, request_id: &str) -> Value {
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
        request_id: request_id.into(),
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
    crate::reviews::merge::merge(store, &job, || {}).unwrap();
    serde_json::to_value(
        store
            .load_review_merge_intent(&job.project_path, &job.request_id)
            .unwrap()
            .unwrap(),
    )
    .unwrap()
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
