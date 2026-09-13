//! The `api::v1` facade as the wire sees it: structured errors beside the
//! string, v1 before legacy, and the QA stream verbs behind the QA flag.

use super::*;

fn project_id_of(state: &mut AppState) -> String {
    let listed = state.handle(req("project.list", json!({})));
    listed["result"]["projects"][0]["project_id"]
        .as_str()
        .expect("a project row")
        .to_string()
}

#[test]
fn an_unknown_method_is_refused_with_a_structured_error() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let refused = state.handle(req("no.such_verb", json!({})));
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "unknown method: no.such_verb");
    assert_eq!(refused["error_code"], "unknown_method");
    assert_eq!(refused["retryable"], false);
    assert_eq!(refused["details"], json!({ "method": "no.such_verb" }));
}

#[test]
fn a_typed_verb_missing_a_required_param_answers_invalid_params() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let refused = state.handle(req("git.diff", json!({ "project_id": project_id })));
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "missing required param: paths");
    assert_eq!(refused["error_code"], "invalid_params");
    assert_eq!(refused["retryable"], false);
    assert!(refused.get("details").is_none(), "{refused}");
}

#[test]
fn a_typed_verb_naming_a_missing_entity_answers_not_found() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let refused = state.handle(req("git.status", json!({ "project_id": "proj-nope" })));
    assert_eq!(refused["error"], "unknown project_id");
    assert_eq!(refused["error_code"], "not_found");
}

#[test]
fn a_success_reply_carries_no_error_fields() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["branch"], "main");
    assert!(status.get("error_code").is_none());
    assert!(status.get("retryable").is_none());
}

#[test]
fn v1_serves_the_git_family_and_the_legacy_route_no_longer_does() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let params = json!({ "project_id": project_id });
    assert!(crate::api::v1::dispatch(&mut state, "git.status", &params).is_some());
    assert!(crate::api::v1::dispatch(&mut state, "fs.list", &json!({})).is_some());
    assert!(crate::api::v1::dispatch(&mut state, "board.list", &json!({})).is_none());
    // ... and the legacy table has no arm left for it, so nothing can answer
    // a git verb twice or drift between the two answers.
    assert!(state.route_legacy("git.status", &params).is_none());
    assert!(state.route_legacy("fs.list", &json!({})).is_none());
    assert!(state.route_legacy("board.list", &json!({})).is_some());
}

#[test]
fn the_qa_stream_verbs_are_unknown_unless_the_qa_agent_is_on() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        false,
        dir.path().join("mcp.sock").display().to_string(),
    );
    for method in ["stream.events", "stream.state"] {
        let refused = state.handle(req(method, json!({ "stream_id": "stream-1" })));
        assert_eq!(
            refused["error_code"], "unknown_method",
            "{method}: {refused}"
        );
        assert_eq!(refused["error"], format!("unknown method: {method}"));
    }
    let mut qa = qa_state(&dir.path().join("repo"), dir.path());
    let refused = qa.handle(req("stream.state", json!({ "stream_id": "stream-1" })));
    assert_eq!(refused["error"], "unknown stream_id", "{refused}");
    assert_eq!(refused["error_code"], "internal");
}
