use super::*;
use crate::api::v1::reviews::{ReviewOpenParams, ReviewReviewer};
use crate::tracker::Actor;

fn open_direct(state: &mut AppState, params: ReviewOpenParams, actor: Actor) -> Value {
    state.review_open(params, actor).unwrap();
    let done = state.take_deferred().unwrap().run();
    state.apply_deferred("tasks.review.open", &Value::Null, done).unwrap()
}

fn params(workspace_id: &str) -> ReviewOpenParams {
    ReviewOpenParams {
        workspace_id: workspace_id.into(), request_id: "open-1".into(),
        title: "Review this change".into(), description: "The proposed change".into(),
        reviewer: None, bases: vec![], excluded_git_directory_ids: vec![],
    }
}

#[test]
fn review_open_publishes_once_and_preserves_creator_and_push_contract() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "opening");
    let params = params(&workspace_id);
    let first = open_direct(&mut state, params.clone(), Actor::User);
    assert_eq!(first["opening_state"], "published");
    assert_eq!(first["task"]["status"], "in_review");
    assert_eq!(first["task"]["created_by"]["kind"], "user");
    assert_eq!(first["task"]["watched"], true);
    assert_eq!(first["reviewer_dispatch"]["state"], "not_requested");
    assert_eq!(first["review"]["snapshots"][0]["author"]["kind"], "user");
    let instruction = &first["push_instructions"][0];
    assert!(instruction["remote"].as_str().unwrap().starts_with("build-review"));
    assert!(instruction["branch"].as_str().unwrap().starts_with("review/"));
    assert_eq!(instruction["refspec"], format!("HEAD:refs/heads/{}", instruction["branch"].as_str().unwrap()));
    let repeated = open_direct(&mut state, params, Actor::User);
    assert_eq!(repeated, first);
}

#[test]
fn reviewer_dispatch_uses_one_stable_operation_and_tracks_the_receiver() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "reviewer");
    let (owner, agent) = project_agent(&mut state, &project);
    let mut asked = params(&workspace_id);
    asked.reviewer = Some(ReviewReviewer::ProjectAgent);
    let first = open_direct(&mut state, asked.clone(), Actor::User);
    assert_eq!(first["reviewer_dispatch"]["state"], "delivered", "{first}");
    assert_eq!(first["task"]["status"], "in_review");
    assert_eq!(first["task"]["trackers"], json!([agent]));
    let operation_id = format!("review-open-{}", first["task"]["id"].as_str().unwrap());
    assert!(state.store.as_ref().unwrap().operation(&operation_id).unwrap().is_some());
    let repeated = open_direct(&mut state, asked, Actor::User);
    assert_eq!(repeated, first);
    let messages = state.handle(req("thread.page", json!({"entity_id":owner, "agent_id":agent, "limit":50})));
    let items = messages["result"]["items"].as_array().unwrap();
    let deliveries: Vec<_> = items.iter().filter(|item| item["type"] == "message" && item["data"]["operation_id"] == operation_id).collect();
    assert_eq!(deliveries.len(), 1, "{messages}");
    let body = deliveries[0]["data"]["body"].as_str().unwrap();
    assert!(body.contains(first["review"]["snapshots"][0]["id"].as_str().unwrap()), "{body}");
}

#[test]
fn foreign_reviewer_is_refused_before_reserving_a_task_or_mutating_git() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "foreign reviewer");
    let (_other_home, other_source) = init_repo();
    let other = added_project(&mut state, &other_source);
    let (_, agent) = project_agent(&mut state, &other);
    let mut asked = params(&workspace_id);
    asked.reviewer = Some(ReviewReviewer::Agent { agent_id: agent });
    let error = state.review_open(asked, Actor::User).unwrap_err();
    assert!(crate::api::ApiError::classify(error).message().contains("not in project"));
    let tasks = state.handle(req("tasks.list", json!({"project_id": project})));
    assert_eq!(tasks["result"]["tasks"], json!([]));
    assert!(state.deferred_work.is_none());
}

#[test]
fn excluded_directory_selection_must_name_an_included_git_directory() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "selection");
    let mut asked = params(&workspace_id);
    asked.excluded_git_directory_ids = vec!["unknown-directory".into()];
    let error = crate::api::ApiError::classify(state.review_open(asked, Actor::User).unwrap_err());
    assert_eq!(error.code(), "invalid_params");
    assert!(state.deferred_work.is_none());
}

#[test]
fn opening_with_agent_creator_preserves_actor_and_device_watch_policy() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "agent creator");
    let (_, agent) = project_agent(&mut state, &project);
    state.watch_agent_filed_tasks = true;
    let actor = Actor::Agent { agent_id: agent };
    let opened = open_direct(&mut state, params(&workspace_id), actor.clone());
    assert_eq!(opened["task"]["created_by"], json!(actor));
    assert_eq!(opened["review"]["snapshots"][0]["author"], json!(actor));
    assert_eq!(opened["task"]["watched"], true);
}
