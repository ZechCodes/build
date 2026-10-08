use super::*;

#[test]
fn deferred_reopen_returns_stale_version_after_another_reopen_and_close() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let workspace_id = workspace(&mut state, &project, "reopen-version");
    let opened = review_call(
        &mut state,
        "tasks.review.open",
        json!({
            "workspace_id":workspace_id,"request_id":"reopen-version",
            "title":"Review version race","description":"Keep the closed review recoverable",
        }),
    );
    let task_id = opened["task"]["id"].as_str().unwrap();
    review_call(
        &mut state,
        "tasks.review.close",
        json!({"task_id":task_id,"expected_version":1,"description":"Deferred"}),
    );
    let asked = json!({"task_id":task_id,"expected_version":2});
    let (admitted, deferred) = state.dispatch_deferring("tasks.review.reopen", &asked);
    assert!(admitted.is_ok());
    let deferred = deferred.unwrap();
    review_call(&mut state, "tasks.review.reopen", asked.clone());
    let closed = review_call(
        &mut state,
        "tasks.review.close",
        json!({"task_id":task_id,"expected_version":3,"description":"Still deferred"}),
    );
    let done = deferred.run();
    let error = state
        .apply_deferred("tasks.review.reopen", &asked, done)
        .unwrap_err();
    let refusal = crate::api::ApiError::classify(error).into_reply(json!(1));
    assert_eq!(refusal["error_code"], "stale_version", "{refusal}");
    assert_eq!(refusal["retryable"], false, "{refusal}");
    assert_eq!(refusal["details"]["task_id"], task_id);
    assert_eq!(refusal["details"]["expected_version"], 2);
    assert_eq!(refusal["details"]["current_version"], 4);
    let after = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(after["review"], closed["review"]);
}

#[test]
fn landed_reopen_version_refusals_keep_versions_after_the_deferred_precheck() {
    for message in [
        "conflict: review version changed: expected 2, found 4",
        "stale_version: review task-1 expected version 2, found 4",
    ] {
        let error = crate::api::v1::reviews::errors::service(
            message.into(),
            json!({"task_id":"task-1","workspace_id":"workspace-1","recovery":"Read tasks.review.get before retrying."}),
        );
        let refusal = crate::api::ApiError::classify(error).into_reply(json!(1));
        assert_eq!(refusal["error_code"], "stale_version", "{refusal}");
        assert_eq!(refusal["details"]["expected_version"], 2, "{refusal}");
        assert_eq!(refusal["details"]["current_version"], 4, "{refusal}");
        assert_eq!(refusal["details"]["workspace_id"], "workspace-1");
        assert!(refusal["details"]["recovery"].is_string());
    }
}
