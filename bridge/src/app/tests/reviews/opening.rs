use super::*;
use crate::api::v1::reviews::{ReviewOpenParams, ReviewReviewer};
use crate::tracker::Actor;

fn open_direct(state: &mut AppState, params: ReviewOpenParams, actor: Actor) -> Value {
    state.review_open(params, actor).unwrap();
    let done = state.take_deferred().unwrap().run();
    state
        .apply_deferred("tasks.review.open", &Value::Null, done)
        .unwrap()
}

fn params(workspace_id: &str) -> ReviewOpenParams {
    ReviewOpenParams {
        workspace_id: workspace_id.into(),
        request_id: "open-1".into(),
        title: "Review this change".into(),
        description: "The proposed change".into(),
        reviewer: None,
        bases: vec![],
        excluded_git_directory_ids: vec![],
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
    assert!(instruction["remote"]
        .as_str()
        .unwrap()
        .starts_with("build-review"));
    assert!(instruction["branch"]
        .as_str()
        .unwrap()
        .starts_with("review/"));
    assert_eq!(
        instruction["refspec"],
        format!(
            "HEAD:refs/heads/{}",
            instruction["branch"].as_str().unwrap()
        )
    );
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
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .operation(&operation_id)
        .unwrap()
        .is_some());
    let repeated = open_direct(&mut state, asked, Actor::User);
    assert_eq!(repeated, first);
    let messages = state.handle(req(
        "thread.page",
        json!({"entity_id":owner, "agent_id":agent, "limit":50}),
    ));
    let items = messages["result"]["items"].as_array().unwrap();
    let deliveries: Vec<_> = items
        .iter()
        .filter(|item| item["type"] == "message" && item["data"]["operation_id"] == operation_id)
        .collect();
    assert_eq!(deliveries.len(), 1, "{messages}");
    let body = deliveries[0]["data"]["body"].as_str().unwrap();
    assert!(
        body.contains(first["review"]["snapshots"][0]["id"].as_str().unwrap()),
        "{body}"
    );
    assert!(body.contains("get_review/read_review"), "{body}");
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
    let error = crate::api::ApiError::classify(state.review_open(asked, Actor::User).unwrap_err());
    assert_eq!(error.code(), "not_found");
    assert_eq!(error.message(), "unknown reviewer agent_id");
    assert_eq!(error.details().unwrap()["reason"], "reviewer_scope");
    assert!(!error.message().contains(&other));
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

#[test]
fn shared_opening_rejects_reviewer_delivery_after_creator_reset() {
    reset_creator_before_reviewer_delivery(true);
}

#[test]
fn settled_opening_rejects_reviewer_delivery_after_creator_reset() {
    reset_creator_before_reviewer_delivery(false);
}

fn reset_creator_before_reviewer_delivery(shared_hooks: bool) {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "reset creator");
    let (owner, creator) = project_agent(&mut app, &project);
    let reviewer = app.handle(req("agent.add", json!({"entity_id": owner})))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let old_thread = app
        .agent_conversation(&owner, Some(&creator))
        .unwrap()
        .id
        .clone();
    let mut asked = params(&workspace_id);
    asked.reviewer = Some(ReviewReviewer::Agent {
        agent_id: reviewer.clone(),
    });
    let shared = Arc::new(Mutex::new(app));
    let job = {
        let mut app = shared.lock().unwrap();
        if shared_hooks {
            app.self_handle = Some(Arc::downgrade(&shared));
        }
        let (answer, job) = app.agent_action_deferring(
            &owner,
            &creator,
            crate::mcp::BridgeAction::TrackerOpenReview { params: asked },
        );
        answer.unwrap();
        let reset = app.handle(req(
            "conversation.reset",
            json!({
                "project_id": project, "entity_id": owner, "agent_id": creator,
                "conversation_id": creator, "expected_thread_id": old_thread,
            }),
        ));
        assert_eq!(reset["ok"], true, "{reset}");
        assert_ne!(
            app.agent_conversation(&owner, Some(&creator)).unwrap().id,
            old_thread,
        );
        job.unwrap()
    };
    let done = job.run();
    let mut app = shared.lock().unwrap();
    let opened = app
        .apply_deferred(crate::app::mcp::MCP_CONTROL_METHOD, &Value::Null, done)
        .unwrap();
    assert_eq!(opened["opening_state"], "published");
    assert_eq!(opened["task"]["status"], "in_review");
    assert_eq!(opened["review"]["snapshots"].as_array().unwrap().len(), 1);
    assert_eq!(opened["reviewer_dispatch"]["state"], "failed", "{opened}");
    assert!(opened["reviewer_dispatch"]["error"]
        .as_str()
        .unwrap()
        .contains("stale thread_id"));
    let task_id = opened["task"]["id"].as_str().unwrap();
    let store = app.store.as_ref().unwrap();
    assert!(store.load_review(task_id).unwrap().is_some());
    assert!(store
        .operation(&format!("review-open-{task_id}"))
        .unwrap()
        .is_none());
    assert!(
        app.agent_conversation(&owner, Some(&reviewer))
            .unwrap()
            .items
            .is_empty(),
        "a stale creator cannot dispatch new reviewer work"
    );
    assert!(
        app.agent_conversation(&owner, Some(&creator))
            .unwrap()
            .items
            .is_empty(),
        "old work cannot repopulate the creator's replacement thread"
    );
}

#[test]
fn reviewer_dispatch_preserves_the_unchanged_creator_generation() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "creator generation");
    let (owner, creator) = project_agent(&mut app, &project);
    let conversation_id = app
        .resolve_conversation_address(&owner, Some(&creator))
        .unwrap()
        .conversation_id;
    let reviewer = app.handle(req("agent.add", json!({"entity_id": owner})))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut asked = params(&workspace_id);
    asked.reviewer = Some(ReviewReviewer::Agent { agent_id: reviewer });
    let opened = open_direct(
        &mut app,
        asked,
        Actor::Agent {
            agent_id: creator.clone(),
        },
    );
    assert_eq!(
        opened["reviewer_dispatch"]["state"], "delivered",
        "{opened}"
    );
    let requester = app
        .store
        .as_ref()
        .unwrap()
        .operation(&format!(
            "review-open-{}",
            opened["task"]["id"].as_str().unwrap()
        ))
        .unwrap()
        .unwrap()
        .requested_by
        .unwrap();
    assert_eq!(requester.entity_id, owner);
    assert_eq!(requester.agent_id, creator);
    assert_eq!(requester.conversation_id, conversation_id);
}

#[test]
fn an_opening_claim_blocks_removal_and_checks_a_new_reclaim_reservation_off_lock() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "opening reservation");
    let shared = Arc::new(Mutex::new(app));
    shared.lock().unwrap().self_handle = Some(Arc::downgrade(&shared));
    let asked = json!(params(&workspace_id));
    let job = {
        let mut app = shared.lock().unwrap();
        let (answer, job) = app.dispatch_deferring("tasks.review.open", &asked);
        assert!(answer.is_ok());
        let removal = app.handle(req(
            "workspace.delete",
            json!({"workspace_id":workspace_id}),
        ));
        assert_eq!(removal["error_code"], "busy", "{removal}");
        app.reserve_workspace_for_test(&workspace_id);
        job.unwrap()
    };
    let done = job.run();
    let error = shared
        .lock()
        .unwrap()
        .apply_deferred("tasks.review.open", &asked, done)
        .unwrap_err();
    assert_eq!(crate::api::ApiError::classify(error).code(), "busy");
    let mut app = shared.lock().unwrap();
    let tasks = app.handle(req("tasks.list", json!({"project_id":project})));
    assert_eq!(tasks["result"]["tasks"], json!([]));
    assert!(app.workspaces.get(&workspace_id).is_some());
}

#[test]
fn a_lost_response_retry_keeps_an_explicit_user_unwatch() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let workspace_id = workspace(&mut app, &project, "retry policy");
    let asked = params(&workspace_id);
    let opened = open_direct(&mut app, asked.clone(), Actor::User);
    let unwatched = app.handle(req(
        "tasks.unwatch",
        json!({"task_id":opened["task"]["id"]}),
    ));
    assert_eq!(unwatched["ok"], true, "{unwatched}");
    let repeated = open_direct(&mut app, asked, Actor::User);
    assert!(!repeated["task"]["watched"].as_bool().unwrap_or(false));
    assert_eq!(repeated["task"]["id"], opened["task"]["id"]);
}

#[test]
fn explicitly_excluded_git_and_live_sources_remain_members_when_their_sources_disappear() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracked(home.path());
    let extra = init_repo_named(home.path(), "unavailable-git");
    let source = app.handle(req(
        "project.add_source",
        json!({"project_id":project,"path":extra,"name":"unavailable"}),
    ));
    assert_eq!(source["ok"], true, "{source}");
    let live = home.path().join("live-source");
    std::fs::create_dir(&live).unwrap();
    let source = app.handle(req(
        "project.add_source",
        json!({"project_id":project,"path":live,"name":"live"}),
    ));
    assert_eq!(source["ok"], true, "{source}");
    let workspace_id = workspace(&mut app, &project, "fixed membership");
    let excluded_id = app
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .iter()
        .find(|directory| directory.source_path == extra)
        .unwrap()
        .id
        .clone();
    std::fs::remove_dir_all(&extra).unwrap();
    std::fs::remove_dir_all(&live).unwrap();
    let mut asked = params(&workspace_id);
    asked.excluded_git_directory_ids.push(excluded_id.clone());
    let opened = open_direct(&mut app, asked, Actor::User);
    let memberships = opened["review"]["pull_request"]["directories"]
        .as_array()
        .unwrap();
    assert_eq!(memberships.len(), 3);
    assert!(memberships
        .iter()
        .any(|member| member["directory_id"] == excluded_id && member["kind"] == "excluded"));
    assert!(memberships.iter().any(|member| member["kind"] == "live"));
    assert_eq!(opened["review"]["bindings"].as_array().unwrap().len(), 1);
}
